//! The OSC 99 *untracked* system leg: a desktop notification freminal shows
//! and then forgets.
//!
//! macOS and Windows give a background thread no way to observe a
//! notification being activated or closed (the macOS callbacks need the main
//! run loop), so on those platforms the system leg is fire-and-forget. The
//! registry models that as [`Osc99Leg::UntrackedSystem`](super::Osc99Leg):
//! it emits [`Osc99Effect::ShowUntrackedSystem`](super::Osc99Effect), answers
//! a `c=1` request with `untracked` at once, and never waits for the leg.
//!
//! The executor shows the notification through an [`Osc99UntrackedBackend`]:
//! `NotifyRustUntrackedBackend` on macOS and Windows, and
//! [`NoUntrackedBackend`] everywhere else (the registry never routes to the
//! untracked leg on Linux/BSD, whose system leg is the zbus service) and as
//! the stand-in when the real backend is busy.

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::borrow::Cow;

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use freminal_common::buffer_states::osc99_notification::Osc99Sound;
use freminal_common::buffer_states::window_manipulation::Notification99Data;

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use super::icon_raster::rasterize_chrome_icon_png;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use crate::gui::icons::standard_icon_glyph;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::icon_tempfiles::{IconTempFiles, default_icon_dir};

/// Shows a notification on the untracked system leg.
///
/// A trait so the executor and its tests reach no OS notification API and so
/// every platform compiles the same executor.
pub trait Osc99UntrackedBackend {
    /// Display `notification` and forget it. There is no handle, no later
    /// event and no way to close it; a failure to display is logged, not
    /// reported, because the protocol has nothing to report it with.
    fn show(&mut self, notification: &Notification99Data);
}

/// The backend that displays nothing.
///
/// Linux/BSD production builds use it (their registry never produces an
/// untracked leg), and every platform uses it as the stand-in when the real
/// backend's cell is busy.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoUntrackedBackend;

impl Osc99UntrackedBackend for NoUntrackedBackend {
    fn show(&mut self, notification: &Notification99Data) {
        tracing::trace!(
            id = ?notification.id,
            "OSC 99: no untracked system backend; notification not shown"
        );
    }
}

/// A backend that records every notification it is handed, for tests.
#[cfg(test)]
#[derive(Debug, Default)]
pub struct RecordingUntrackedBackend {
    /// Every notification shown, in call order.
    pub shown: Vec<Notification99Data>,
}

#[cfg(test)]
impl Osc99UntrackedBackend for RecordingUntrackedBackend {
    fn show(&mut self, notification: &Notification99Data) {
        self.shown.push(notification.clone());
    }
}

/// What an OSC 99 `s=` sound becomes on the untracked leg.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99PlatformSound {
    /// Play the platform's default notification sound.
    PlatformDefault,
    /// Play nothing.
    Silent,
}

/// The sound the untracked leg plays for `sound`.
///
/// `system` is the platform default and `silent` is silence. A named sound
/// (`s=<name>`) is a freedesktop theme name that neither platform knows, so it
/// falls back to the platform default rather than to silence, which would drop
/// a sound the program asked for.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[must_use]
pub const fn untracked_sound(sound: &Osc99Sound) -> Osc99PlatformSound {
    match sound {
        Osc99Sound::System | Osc99Sound::Named(_) => Osc99PlatformSound::PlatformDefault,
        Osc99Sound::Silent => Osc99PlatformSound::Silent,
    }
}

/// The side, in pixels, of the image a standard icon name is drawn to.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const STANDARD_ICON_SIZE_PX: u32 = 256;

/// The icon image to show for `data`, as encoded image bytes.
///
/// Names come first, as on Linux: the first `n=` name that is one of the OSC 99
/// standard icon names is drawn from the bundled glyph (see
/// [`rasterize_chrome_icon_png`]). Neither platform resolves a freedesktop icon
/// name itself, so a name that is not a standard one is skipped. If no name is
/// standard, or the glyph cannot be drawn, the transmitted icon data is used;
/// with neither there is no image.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn untracked_icon_image(data: &Notification99Data) -> Option<Cow<'_, [u8]>> {
    data.icon_names
        .iter()
        .find_map(|name| standard_icon_glyph(name))
        .and_then(|icon| rasterize_chrome_icon_png(icon, STANDARD_ICON_SIZE_PX))
        .map(Cow::Owned)
        .or_else(|| data.icon_data.as_deref().map(Cow::Borrowed))
}

/// The backend for the platform freminal is running on.
#[must_use]
pub fn platform_untracked_backend() -> Box<dyn Osc99UntrackedBackend> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Box::<NotifyRustUntrackedBackend>::default()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(NoUntrackedBackend)
    }
}

/// The name the platform's notification API knows its default sound by.
///
/// macOS: `mac-notification-sys` turns exactly this string into
/// `NSUserNotificationDefaultSoundName`. Windows: `winrt-notification` parses
/// `"Default"` as `Sound::Default`.
#[cfg(any(target_os = "macos", target_os = "windows"))]
const PLATFORM_DEFAULT_SOUND_NAME: &str = if cfg!(target_os = "macos") {
    "NSUserNotificationDefaultSoundName"
} else {
    "Default"
};

/// Build the `notify-rust` notification for `data`.
///
/// - The summary is the title (the body when the title is empty); the app
///   name is `app_name` or `freminal`.
/// - No action buttons are registered: their activation cannot be observed.
/// - The sound follows [`untracked_sound`]. Setting no sound name is silent on
///   both platforms.
/// - Urgency is set on Windows only (`Critical` becomes the persistent
///   "reminder" toast). `notify-rust` has no `urgency` on macOS.
/// - `w=` becomes the timeout. Windows honours it coarsely (short or long
///   toast); macOS ignores it.
/// - The icon image (a drawn standard icon name or transmitted bytes, see
///   [`untracked_icon_image`]) is shown from `icon_path`, the file the caller
///   wrote for it. With no image, the first transmitted icon name is set as
///   the icon, which neither platform currently draws; only the standard names
///   can be shown on these platforms.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn build_untracked_notification(
    data: &Notification99Data,
    icon_path: Option<&std::path::Path>,
) -> notify_rust::Notification {
    use freminal_common::buffer_states::osc99_notification::Osc99Expiry;

    let body = data.body.as_deref().unwrap_or_default();
    let mut notification = notify_rust::Notification::new();
    notification
        .appname(data.app_name.as_deref().unwrap_or("freminal"))
        .summary(if data.title.is_empty() {
            body
        } else {
            &data.title
        })
        .body(body);

    #[cfg(target_os = "windows")]
    {
        use freminal_common::buffer_states::osc_notify_99::NotificationUrgency;

        notification.urgency(match data.urgency {
            NotificationUrgency::Low => notify_rust::Urgency::Low,
            NotificationUrgency::Normal => notify_rust::Urgency::Normal,
            NotificationUrgency::Critical => notify_rust::Urgency::Critical,
        });
    }

    match untracked_sound(&data.sound) {
        Osc99PlatformSound::PlatformDefault => {
            notification.sound_name(PLATFORM_DEFAULT_SOUND_NAME);
        }
        Osc99PlatformSound::Silent => {}
    }

    notification.timeout(match data.expiry {
        Osc99Expiry::OsDefault => notify_rust::Timeout::Default,
        Osc99Expiry::Never => notify_rust::Timeout::Never,
        Osc99Expiry::After(ms) => notify_rust::Timeout::Milliseconds(ms.get()),
    });

    if let Some(path) = icon_path {
        if let Some(path) = path.to_str() {
            notification.image_path(path);
        }
    } else if data.icon_data.is_none()
        && let Some(first) = data.icon_names.first()
    {
        notification.icon(first);
    }

    notification
}

/// Show `notification` and drop the handle `show()` returns.
///
/// Dropping the handle is what shows a macOS notification:
/// `NotificationHandle`'s `Drop` sends it with `asynchronous: true`. That send
/// still waits (up to 2 s, bounded) for the OS to confirm delivery, which is
/// why macOS runs this on a worker thread ([`deliver_on_thread`]); Windows calls it inline. On Windows the toast
/// is already showing when `show()` returns, and the handle only holds the
/// event receiver nobody reads.
#[cfg(any(target_os = "macos", target_os = "windows"))]
fn show_and_forget(notification: &notify_rust::Notification) {
    match notification.show() {
        Ok(handle) => drop(handle),
        Err(e) => tracing::warn!("failed to show OSC 99 desktop notification: {e}"),
    }
}

/// Deliver `notification` on a short-lived thread.
///
/// The thread exits as soon as `show()` and the handle drop return. It never
/// waits for the user, so it cannot pile up. `notify_rust::Notification` is
/// plain owned data (strings, a `Vec<String>`, a `Timeout`, an `Option<u32>`)
/// and so is `Send`. If the thread cannot be spawned the notification is
/// dropped, not shown.
#[cfg(target_os = "macos")]
fn deliver_on_thread(notification: notify_rust::Notification) {
    let builder = std::thread::Builder::new().name("freminal-osc99-deliver".to_owned());
    if let Err(e) = builder.spawn(move || show_and_forget(&notification)) {
        tracing::warn!("failed to spawn OSC 99 delivery thread; notification not shown: {e}");
    }
}

/// The macOS and Windows backend: fire-and-forget `notify-rust`.
///
/// The notification is built on the GUI thread, which keeps the icon
/// temp-file store single-threaded; only the built notification crosses to
/// the macOS delivery thread. The backend owns the icon store, and removes the
/// files when it is dropped (the GUI drops it when the application shuts
/// down; the windowing layer has no exit hook of its own).
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug)]
pub struct NotifyRustUntrackedBackend {
    /// The icon files written for notifications shown so far.
    icons: IconTempFiles,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Default for NotifyRustUntrackedBackend {
    fn default() -> Self {
        Self {
            icons: IconTempFiles::with_dir(default_icon_dir()),
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Osc99UntrackedBackend for NotifyRustUntrackedBackend {
    fn show(&mut self, data: &Notification99Data) {
        let icon_path =
            untracked_icon_image(data).and_then(|bytes| match self.icons.path_for(&bytes) {
                Ok(path) => Some(path),
                Err(e) => {
                    tracing::warn!("failed to write OSC 99 icon temp file: {e}");
                    None
                }
            });

        let notification = build_untracked_notification(data, icon_path.as_deref());
        // macOS: off the GUI thread, see `deliver_on_thread`. Windows: inline,
        // its `show()` returns immediately.
        #[cfg(target_os = "macos")]
        deliver_on_thread(notification);
        #[cfg(target_os = "windows")]
        show_and_forget(&notification);
    }
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Drop for NotifyRustUntrackedBackend {
    fn drop(&mut self) {
        self.icons.remove_all();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn sound_system_is_the_platform_default() {
        assert_eq!(
            untracked_sound(&Osc99Sound::System),
            Osc99PlatformSound::PlatformDefault
        );
    }

    #[test]
    fn sound_silent_is_silent() {
        assert_eq!(
            untracked_sound(&Osc99Sound::Silent),
            Osc99PlatformSound::Silent
        );
    }

    #[test]
    fn a_named_sound_falls_back_to_the_platform_default() {
        assert_eq!(
            untracked_sound(&Osc99Sound::Named("message-new-instant".to_owned())),
            Osc99PlatformSound::PlatformDefault
        );
    }

    #[test]
    fn the_no_op_backend_accepts_a_notification() {
        let mut backend = NoUntrackedBackend;
        backend.show(&test_notification());
    }

    #[test]
    fn the_recording_backend_records_in_order() {
        let mut backend = RecordingUntrackedBackend::default();
        let first = test_notification();
        let second = Notification99Data {
            title: "Second".to_owned(),
            ..test_notification()
        };

        backend.show(&first);
        backend.show(&second);

        assert_eq!(backend.shown, [first, second]);
    }

    fn decoded_size(image: &[u8]) -> Option<(u32, u32)> {
        image::load_from_memory(image)
            .ok()
            .map(|decoded| (decoded.width(), decoded.height()))
    }

    #[test]
    fn a_standard_name_beats_transmitted_data() {
        let data = Notification99Data {
            icon_names: vec!["info".to_owned()],
            icon_data: Some(vec![1, 2, 3]),
            ..test_notification()
        };

        let image = untracked_icon_image(&data).unwrap();

        assert_eq!(decoded_size(&image), Some((256, 256)));
    }

    #[test]
    fn the_first_standard_name_wins_over_non_standard_names_before_it() {
        let drawn = |names: &[&str]| {
            untracked_icon_image(&Notification99Data {
                icon_names: names.iter().map(|name| (*name).to_owned()).collect(),
                ..test_notification()
            })
            .unwrap()
            .into_owned()
        };

        assert_eq!(drawn(&["weather-clear", "error"]), drawn(&["error"]));
        assert_ne!(drawn(&["error", "info"]), drawn(&["info"]));
    }

    #[test]
    fn a_non_standard_name_falls_through_to_transmitted_data() {
        let data = Notification99Data {
            icon_names: vec!["weather-clear".to_owned()],
            icon_data: Some(vec![1, 2, 3]),
            ..test_notification()
        };

        assert_eq!(untracked_icon_image(&data).unwrap().as_ref(), [1, 2, 3]);
    }

    #[test]
    fn no_names_and_data_is_the_data() {
        let data = Notification99Data {
            icon_data: Some(vec![9, 8, 7]),
            ..test_notification()
        };

        assert_eq!(untracked_icon_image(&data).unwrap().as_ref(), [9, 8, 7]);
    }

    #[test]
    fn a_non_standard_name_without_data_has_no_image() {
        let data = Notification99Data {
            icon_names: vec!["weather-clear".to_owned()],
            ..test_notification()
        };

        assert_eq!(untracked_icon_image(&data), None);
        assert_eq!(untracked_icon_image(&test_notification()), None);
    }

    fn test_notification() -> Notification99Data {
        use freminal_common::buffer_states::osc_notify_99::{
            NotificationOccasion, NotificationUrgency,
        };
        use freminal_common::buffer_states::osc99_notification::{
            Osc99ActivationFocus, Osc99ActivationReporting, Osc99CloseReporting, Osc99Expiry,
        };

        Notification99Data {
            id: Some("n".to_owned()),
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

    /// The builder only exists on macOS and Windows; these run there.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    mod builder {
        use super::*;
        use freminal_common::buffer_states::osc99_notification::Osc99Expiry;

        #[test]
        fn expiry_maps_to_the_timeout() {
            let timeout_for = |expiry| {
                build_untracked_notification(
                    &Notification99Data {
                        expiry,
                        ..test_notification()
                    },
                    None,
                )
                .timeout
            };

            assert_eq!(
                timeout_for(Osc99Expiry::OsDefault),
                notify_rust::Timeout::Default
            );
            assert_eq!(timeout_for(Osc99Expiry::Never), notify_rust::Timeout::Never);
            assert_eq!(
                timeout_for(Osc99Expiry::After(std::num::NonZeroU32::new(5000).unwrap())),
                notify_rust::Timeout::Milliseconds(5000)
            );
        }

        #[test]
        fn the_title_is_the_summary_and_the_body_stands_in_for_an_empty_title() {
            let titled = build_untracked_notification(
                &Notification99Data {
                    body: Some("Body".to_owned()),
                    ..test_notification()
                },
                None,
            );
            assert_eq!(titled.summary, "Title");
            assert_eq!(titled.body, "Body");

            let untitled = build_untracked_notification(
                &Notification99Data {
                    title: String::new(),
                    body: Some("Body".to_owned()),
                    ..test_notification()
                },
                None,
            );
            assert_eq!(untitled.summary, "Body");
        }

        #[test]
        fn no_buttons_are_registered() {
            let notification = build_untracked_notification(
                &Notification99Data {
                    buttons: vec!["Yes".to_owned(), "No".to_owned()],
                    ..test_notification()
                },
                None,
            );
            assert_eq!(notification.actions, Vec::<String>::new());
        }

        #[test]
        fn a_transmitted_icon_name_is_used_but_never_the_freminal_icon() {
            let named = build_untracked_notification(
                &Notification99Data {
                    icon_names: vec!["weather-clear".to_owned()],
                    ..test_notification()
                },
                None,
            );
            assert_eq!(named.icon, "weather-clear");

            // Transmitted bytes are shown from the temp file, so the icon
            // name stays empty (and is certainly not "freminal").
            let with_bytes = build_untracked_notification(
                &Notification99Data {
                    icon_data: Some(vec![1, 2, 3]),
                    icon_names: vec!["weather-clear".to_owned()],
                    ..test_notification()
                },
                Some(std::path::Path::new("icon.png")),
            );
            assert_eq!(with_bytes.icon, "");
        }
    }
}
