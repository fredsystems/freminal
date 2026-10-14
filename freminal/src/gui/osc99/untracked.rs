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
//! `PlatformUntrackedBackend` on macOS and Windows, and
//! [`NoUntrackedBackend`] everywhere else (the registry never routes to the
//! untracked leg on Linux/BSD, whose system leg is the zbus service) and as
//! the stand-in when the real backend is busy.
//!
//! # Delivery worker
//!
//! The GUI thread never touches the disk, an image decoder or an OS
//! notification API. `PlatformUntrackedBackend::show` only builds a plain
//! `DeliveryRequest` and `try_send`s it to one long-lived thread,
//! `freminal-osc99-deliver`, spawned on the first notification. The queue holds
//! `QUEUE_CAPACITY` requests; when it is full the new request is dropped (and
//! logged at debug), so a program that floods notifications cannot grow memory
//! or stall the GUI. The worker owns the icon temp-file store
//! (`IconTempFiles`), rasterises each standard icon once per process,
//! re-encodes transmitted icon bytes (`reencode_icon_png`) so the OS never
//! decodes a file a program wrote, and shows the notification. It exits when
//! the backend is dropped, removing its files.
//!
//! # Platforms
//!
//! - **macOS** uses `notify-rust`, which wraps `mac-notification-sys`.
//!   `show()` never fails there: it only wraps the notification in a handle,
//!   and the send happens when the handle drops, asynchronously. That send
//!   waits up to 2 s for the OS to confirm delivery, and `notify-rust`
//!   discards any error. A delivery failure (notifications disabled for the
//!   app, for example) is therefore **not observable** by freminal, and the
//!   warning for a failed `show()` effectively never fires. The app name and
//!   the timeout have no effect on macOS.
//! - **Windows** builds the toast directly with `tauri-winrt-notification`, so
//!   the image is an `appLogoOverride`. `notify-rust` only offers an inline
//!   image there, and maps the `Critical` urgency to a "reminder" toast; this
//!   leg does neither. The app name is unused: toasts are attributed to the
//!   PowerShell app id, because freminal has no `AppUserModelID` of its own.

use freminal_common::buffer_states::osc99_notification::Notification99Data;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use freminal_common::buffer_states::osc99_notification::{Osc99Expiry, Osc99Sound};

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::collections::HashMap;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::thread::JoinHandle;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use std::time::{Duration, Instant};

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use crossbeam_channel::{Receiver, Sender, TrySendError};

#[cfg(any(target_os = "macos", target_os = "windows", test))]
use super::icon_decode::reencode_icon_png;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use super::icon_raster::rasterize_chrome_icon_png;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use super::icon_tempfiles::IconTempFiles;
#[cfg(any(target_os = "macos", target_os = "windows"))]
use super::icon_tempfiles::default_icon_dir;
#[cfg(any(target_os = "macos", target_os = "windows", test))]
use crate::gui::icons::{ChromeIcon, standard_icon_glyph};

/// Shows a notification on the untracked system leg.
///
/// A trait so the executor and its tests reach no OS notification API and so
/// every platform compiles the same executor.
pub trait Osc99UntrackedBackend {
    /// Display `notification` and forget it. There is no handle, no later
    /// event and no way to close it; a failure to display is logged, not
    /// reported, because the protocol has nothing to report it with. (On
    /// macOS a failure is not even observable; see the module docs.)
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

/// The icon a notification is shown with, chosen on the GUI thread and
/// resolved to a file on the worker.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
enum UntrackedIcon {
    /// One of the OSC 99 standard icon names, drawn from the bundled glyph.
    Standard(ChromeIcon),
    /// Icon bytes the program transmitted, not yet decoded.
    Transmitted(Vec<u8>),
}

/// The icon to show for `data`, if any.
///
/// Names come first, as on Linux: the first `n=` name that is one of the OSC 99
/// standard icon names is drawn from the bundled glyph (see
/// [`rasterize_chrome_icon_png`]). Neither platform resolves a freedesktop icon
/// name itself, so a name that is not a standard one is skipped. If no name is
/// standard, the transmitted icon data is used; with neither there is no image.
/// A standard name whose glyph then cannot be drawn gives no image (the glyph
/// is bundled, so that does not happen outside a broken build).
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn untracked_icon_choice(data: &Notification99Data) -> Option<UntrackedIcon> {
    data.icon_names
        .iter()
        .find_map(|name| standard_icon_glyph(name))
        .map(UntrackedIcon::Standard)
        .or_else(|| data.icon_data.clone().map(UntrackedIcon::Transmitted))
}

/// Everything the worker needs to show one notification: plain owned data,
/// so building it on the GUI thread costs a few string copies and nothing
/// else.
///
/// There is no app name (neither platform uses it, see the module docs) and no
/// urgency (the Windows leg no longer maps it to anything).
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq)]
struct DeliveryRequest {
    /// The title (`d=0` payload); may be empty.
    title: String,
    /// The body; `None` when the program sent none.
    body: Option<String>,
    /// The sound to play.
    sound: Osc99PlatformSound,
    /// `w=`: macOS ignores it, Windows maps it to a short or long toast.
    expiry: Osc99Expiry,
    /// The icon, if any.
    icon: Option<UntrackedIcon>,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl DeliveryRequest {
    /// The request that shows `data`.
    fn from_notification(data: &Notification99Data) -> Self {
        Self {
            title: data.title.clone(),
            body: data.body.clone(),
            sound: untracked_sound(&data.sound),
            expiry: data.expiry,
            icon: untracked_icon_choice(data),
        }
    }

    /// The headline and the detail line: the title and the body, or just the
    /// body as the headline when the title is empty (it is not repeated as the
    /// detail).
    fn lines(&self) -> (&str, Option<&str>) {
        match (self.title.as_str(), self.body.as_deref()) {
            ("", body) => (body.unwrap_or_default(), None),
            (title, body) => (title, body),
        }
    }
}

/// How many requests may wait for the worker. A request that arrives when the
/// queue is full is dropped.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const QUEUE_CAPACITY: usize = 16;

/// How long [`DeliveryWorker::shutdown`] waits for the worker to finish when
/// the backend is dropped. The worker drains what is queued first, so on macOS
/// (up to 2 s per notification) it can take longer than this; the wait is then
/// abandoned and the worker finishes, or the process exits, on its own.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
const SHUTDOWN_WAIT: Duration = Duration::from_millis(500);

/// What happened to a request offered to the worker.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Submit {
    /// The request is queued.
    Queued,
    /// The queue was full; the request was dropped.
    DroppedFull,
    /// The worker has gone (its thread ended); the request was dropped.
    WorkerGone,
}

/// Offer `request` to `queue` without ever blocking.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn try_enqueue(queue: &Sender<DeliveryRequest>, request: DeliveryRequest) -> Submit {
    match queue.try_send(request) {
        Ok(()) => Submit::Queued,
        Err(TrySendError::Full(_)) => {
            tracing::debug!(
                "OSC 99: untracked delivery queue is full ({QUEUE_CAPACITY}); notification dropped"
            );
            Submit::DroppedFull
        }
        Err(TrySendError::Disconnected(_)) => {
            tracing::debug!("OSC 99: untracked delivery worker has stopped; notification dropped");
            Submit::WorkerGone
        }
    }
}

/// Turns a request's icon into a file path: the worker's half of the icon
/// story.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug)]
struct IconPreparer {
    /// The temp files written so far.
    icons: IconTempFiles,
    /// The PNG drawn for each standard icon, computed at most once per
    /// process. `None` records a glyph that could not be drawn.
    drawn: HashMap<ChromeIcon, Option<Vec<u8>>>,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl IconPreparer {
    fn new(icons: IconTempFiles) -> Self {
        Self {
            icons,
            drawn: HashMap::new(),
        }
    }

    /// The file holding `icon`'s image, or `None` for no icon or when the
    /// image cannot be had. Standard icons are drawn once and cached;
    /// transmitted bytes are decoded within the shared limits and re-encoded
    /// as PNG, so undecodable bytes give no image. Failures are logged at
    /// debug: the notification is shown without the image.
    fn image_path(&mut self, icon: Option<&UntrackedIcon>) -> Option<PathBuf> {
        let Self { icons, drawn } = self;
        let written = match icon? {
            UntrackedIcon::Standard(standard) => {
                let png = drawn
                    .entry(*standard)
                    .or_insert_with(|| rasterize_chrome_icon_png(*standard, STANDARD_ICON_SIZE_PX))
                    .as_deref()?;
                icons.path_for(png)
            }
            UntrackedIcon::Transmitted(bytes) => {
                let Some(png) = reencode_icon_png(bytes) else {
                    tracing::debug!(
                        "OSC 99: transmitted icon did not decode within the limits; no image"
                    );
                    return None;
                };
                icons.path_for(&png)
            }
        };
        match written {
            Ok(path) => Some(path),
            Err(e) => {
                tracing::debug!("OSC 99: icon temp file unavailable; no image: {e}");
                None
            }
        }
    }
}

/// The worker loop: prepare each request's icon, show it, and on the sender's
/// disconnection (after the queue is drained) remove the temp files.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
fn deliver_loop(
    queue: &Receiver<DeliveryRequest>,
    icons: IconTempFiles,
    mut show: impl FnMut(&DeliveryRequest, Option<&Path>),
) {
    let mut preparer = IconPreparer::new(icons);
    while let Ok(request) = queue.recv() {
        let image = preparer.image_path(request.icon.as_ref());
        show(&request, image.as_deref());
    }
    preparer.icons.remove_all();
}

/// The delivery thread and the bounded queue that feeds it.
#[cfg(any(target_os = "macos", target_os = "windows", test))]
#[derive(Debug)]
struct DeliveryWorker {
    /// The queue's sender; `None` once [`Self::shutdown`] has dropped it,
    /// which is what makes the worker exit.
    queue: Option<Sender<DeliveryRequest>>,
    /// The thread; `None` once it has been joined or abandoned.
    thread: Option<JoinHandle<()>>,
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl DeliveryWorker {
    /// Spawn the `freminal-osc99-deliver` thread. `show` runs on it, once per
    /// request, with the path of the request's image if it has one.
    ///
    /// # Errors
    ///
    /// Returns the I/O error if the thread cannot be spawned.
    fn spawn(
        icons: IconTempFiles,
        show: impl FnMut(&DeliveryRequest, Option<&Path>) + Send + 'static,
    ) -> std::io::Result<Self> {
        let (queue, requests) = crossbeam_channel::bounded(QUEUE_CAPACITY);
        let thread = std::thread::Builder::new()
            .name("freminal-osc99-deliver".to_owned())
            .spawn(move || deliver_loop(&requests, icons, show))?;
        Ok(Self {
            queue: Some(queue),
            thread: Some(thread),
        })
    }

    /// Offer `request` to the worker without blocking.
    fn submit(&self, request: DeliveryRequest) -> Submit {
        self.queue
            .as_ref()
            .map_or(Submit::WorkerGone, |queue| try_enqueue(queue, request))
    }

    /// Stop the worker and wait up to `limit` for it to finish.
    ///
    /// Dropping the sender ends the worker's loop once it has drained the
    /// queue; it then removes its temp files. If it has not finished by
    /// `limit` the thread is left to finish on its own.
    fn shutdown(&mut self, limit: Duration) {
        drop(self.queue.take());
        let Some(thread) = self.thread.take() else {
            return;
        };
        let deadline = Instant::now() + limit;
        while !thread.is_finished() {
            if Instant::now() >= deadline {
                tracing::debug!("OSC 99: untracked delivery worker still busy at shutdown");
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        if thread.join().is_err() {
            tracing::debug!("OSC 99: untracked delivery worker had panicked");
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "windows", test))]
impl Drop for DeliveryWorker {
    fn drop(&mut self) {
        self.shutdown(SHUTDOWN_WAIT);
    }
}

/// The backend for the platform freminal is running on.
#[must_use]
pub fn platform_untracked_backend() -> Box<dyn Osc99UntrackedBackend> {
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        Box::<PlatformUntrackedBackend>::default()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Box::new(NoUntrackedBackend)
    }
}

/// The name the platform's notification API knows its default sound by.
///
/// `mac-notification-sys` turns exactly this string into
/// `NSUserNotificationDefaultSoundName` (`objc/notify.m` compares the sound
/// option to it).
#[cfg(any(target_os = "macos", test))]
const MACOS_DEFAULT_SOUND_NAME: &str = "NSUserNotificationDefaultSoundName";

/// Build the `notify-rust` notification for `request` (macOS).
///
/// - The summary is the headline, the body the detail line
///   ([`DeliveryRequest::lines`]).
/// - No action buttons are registered: their activation cannot be observed.
/// - The sound follows [`untracked_sound`]. Setting no sound name is silent.
/// - `w=` becomes the timeout, which macOS ignores.
/// - `icon_path`, the file the worker wrote for the request's image, is the
///   notification's content image. `notify-rust` cannot set an icon on macOS,
///   so a transmitted icon *name* has nowhere to go and is not used.
///
/// Built in test builds on every platform so its logic is tested where the
/// suite runs; `notify-rust`'s `Notification` has the same public fields and
/// builder methods used here everywhere.
#[cfg(any(target_os = "macos", test))]
fn build_macos_notification(
    request: &DeliveryRequest,
    icon_path: Option<&Path>,
) -> notify_rust::Notification {
    let (headline, detail) = request.lines();
    let mut notification = notify_rust::Notification::new();
    notification
        .summary(headline)
        .body(detail.unwrap_or_default());

    match request.sound {
        Osc99PlatformSound::PlatformDefault => {
            notification.sound_name(MACOS_DEFAULT_SOUND_NAME);
        }
        Osc99PlatformSound::Silent => {}
    }

    notification.timeout(match request.expiry {
        Osc99Expiry::OsDefault => notify_rust::Timeout::Default,
        Osc99Expiry::Never => notify_rust::Timeout::Never,
        Osc99Expiry::After(ms) => notify_rust::Timeout::Milliseconds(ms.get()),
    });

    if let Some(path) = icon_path.and_then(Path::to_str) {
        notification.image_path(path);
    }

    notification
}

/// Show `request` on macOS and forget it.
///
/// `notify-rust`'s macOS `show()` only wraps the notification in a handle and
/// cannot fail. The send happens when the handle is dropped: it is
/// asynchronous (it does not wait for the user) but still waits up to 2 s for
/// the OS to confirm delivery, which is why this runs on the delivery worker
/// and never on the GUI thread. `notify-rust` discards any error from that
/// send, so a delivery failure is not observable here.
#[cfg(target_os = "macos")]
fn show_on_platform(request: &DeliveryRequest, icon_path: Option<&Path>) {
    match build_macos_notification(request, icon_path).show() {
        Ok(handle) => drop(handle),
        Err(e) => tracing::warn!("failed to show OSC 99 desktop notification: {e}"),
    }
}

/// How long a Windows toast stays up.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastLength {
    /// The short toast (about 7 s).
    Short,
    /// The long toast (about 25 s).
    Long,
}

/// The toast length for `w=`: Windows only has a short and a long toast, so a
/// toast that should never expire, or for 25 s or more, is the long one.
#[cfg(any(target_os = "windows", test))]
const fn toast_length(expiry: Osc99Expiry) -> ToastLength {
    match expiry {
        Osc99Expiry::Never => ToastLength::Long,
        Osc99Expiry::After(ms) if ms.get() >= 25_000 => ToastLength::Long,
        Osc99Expiry::OsDefault | Osc99Expiry::After(_) => ToastLength::Short,
    }
}

/// The path to hand `tauri-winrt-notification`'s `Toast::icon` so the toast's
/// `file:///` URI is valid, or `None` if no valid one can be made.
///
/// The crate builds the image source as `format!("file:///{}", path.display())`
/// and puts it in an XML attribute, which the XML DOM escapes. It does **not**
/// percent-encode the path, so a space (a Windows user name with a space puts
/// one in `%TEMP%`) or a `%` is left for the URI parser to interpret. The path
/// is therefore encoded here: every byte outside `A-Z a-z 0-9 - . _ ~` becomes
/// `%XX` (UTF-8 bytes for non-ASCII), `\` becomes `/`, and the drive colon is
/// kept. The result is a valid URI path whatever the temp directory is called.
///
/// The crate documents that the path must be absolute and must not have a UNC
/// prefix, so anything that is not `<drive letter>:` followed by a separator
/// (UNC `\\server\share`, `\\?\C:\`, a relative path, a non-UTF-8 path) gives
/// `None` and the toast has no image.
#[cfg(any(target_os = "windows", test))]
fn toast_file_path(path: &Path) -> Option<PathBuf> {
    use std::fmt::Write as _;

    let text = path.to_str()?;
    let rooted_on_a_drive = matches!(
        text.as_bytes(),
        [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic()
    );
    if !rooted_on_a_drive {
        return None;
    }
    let mut encoded = String::with_capacity(text.len());
    for (index, byte) in text.bytes().enumerate() {
        match byte {
            b'\\' | b'/' => encoded.push('/'),
            b':' if index == 1 => encoded.push(':'),
            b'-' | b'.' | b'_' | b'~' => encoded.push(char::from(byte)),
            other if other.is_ascii_alphanumeric() => encoded.push(char::from(other)),
            other => {
                // Writing to a `String` cannot fail.
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    Some(PathBuf::from(encoded))
}

/// Show `request` as a Windows toast and forget it.
///
/// Built directly with `tauri-winrt-notification`, attributed to the
/// PowerShell app id (the only id available without an installed
/// `AppUserModelID`). The image is the `appLogoOverride` square logo. There is
/// no scenario: the `Critical` urgency is deliberately not turned into a
/// persistent "reminder" toast. `show()` returns once the toast has been handed
/// to Windows.
#[cfg(target_os = "windows")]
fn show_on_platform(request: &DeliveryRequest, icon_path: Option<&Path>) {
    use tauri_winrt_notification::{Duration as ToastDuration, IconCrop, Sound, Toast};

    let (headline, detail) = request.lines();
    let mut toast = Toast::new(Toast::POWERSHELL_APP_ID).title(headline);
    if let Some(detail) = detail {
        toast = toast.text1(detail);
    }
    toast = toast
        .duration(match toast_length(request.expiry) {
            ToastLength::Short => ToastDuration::Short,
            ToastLength::Long => ToastDuration::Long,
        })
        .sound(match request.sound {
            Osc99PlatformSound::PlatformDefault => Some(Sound::Default),
            Osc99PlatformSound::Silent => None,
        });

    if let Some(path) = icon_path {
        if let Some(uri_path) = toast_file_path(path) {
            toast = toast.icon(&uri_path, IconCrop::Square, "");
        } else {
            tracing::debug!(
                "OSC 99: icon path {} cannot be a toast image source; no image",
                path.display()
            );
        }
    }

    if let Err(e) = toast.show() {
        tracing::warn!("failed to show OSC 99 desktop notification: {e}");
    }
}

/// The macOS and Windows backend: requests go to the delivery worker.
///
/// The worker is spawned on the first notification and stopped when the
/// backend is dropped (the GUI drops it when the application shuts down; the
/// windowing layer has no exit hook of its own), which also removes the icon
/// files. A worker that has died (its thread panicked) is replaced on the next
/// notification.
#[cfg(any(target_os = "macos", target_os = "windows"))]
#[derive(Debug, Default)]
pub struct PlatformUntrackedBackend {
    /// The delivery worker, once spawned.
    worker: Option<DeliveryWorker>,
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
impl Osc99UntrackedBackend for PlatformUntrackedBackend {
    fn show(&mut self, data: &Notification99Data) {
        let request = DeliveryRequest::from_notification(data);

        if self.worker.is_none() {
            match DeliveryWorker::spawn(
                IconTempFiles::with_dir(default_icon_dir()),
                show_on_platform,
            ) {
                Ok(worker) => self.worker = Some(worker),
                Err(e) => {
                    tracing::warn!(
                        "failed to spawn OSC 99 delivery thread; notification not shown: {e}"
                    );
                    return;
                }
            }
        }

        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.submit(request) == Submit::WorkerGone)
        {
            self.worker = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::num::NonZeroU32;

    fn test_notification() -> Notification99Data {
        use freminal_common::buffer_states::osc99_notification::{
            NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
            Osc99ActivationReporting, Osc99CloseReporting,
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

    fn request(title: &str) -> DeliveryRequest {
        DeliveryRequest {
            title: title.to_owned(),
            body: None,
            sound: Osc99PlatformSound::PlatformDefault,
            expiry: Osc99Expiry::OsDefault,
            icon: None,
        }
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let pixels = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 20, 30, 255]));
        let mut bytes = Vec::new();
        pixels
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .unwrap();
        bytes
    }

    fn decoded_size(path: &Path) -> (u32, u32) {
        let decoded = image::open(path).unwrap();
        (decoded.width(), decoded.height())
    }

    fn private_store() -> (tempfile::TempDir, IconTempFiles) {
        let root = tempfile::tempdir().unwrap();
        let store = IconTempFiles::with_dir(root.path().join("icons"));
        (root, store)
    }

    // ── sounds and the backends that show nothing ───────────────────────

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

    // ── the icon choice ─────────────────────────────────────────────────

    #[test]
    fn a_standard_name_beats_transmitted_data() {
        let data = Notification99Data {
            icon_names: vec!["info".to_owned()],
            icon_data: Some(vec![1, 2, 3]),
            ..test_notification()
        };

        assert_eq!(
            untracked_icon_choice(&data),
            Some(UntrackedIcon::Standard(ChromeIcon::Info))
        );
    }

    #[test]
    fn the_first_standard_name_wins_over_non_standard_names_before_it() {
        let chosen = |names: &[&str]| {
            untracked_icon_choice(&Notification99Data {
                icon_names: names.iter().map(|name| (*name).to_owned()).collect(),
                ..test_notification()
            })
        };

        assert_eq!(
            chosen(&["weather-clear", "error"]),
            Some(UntrackedIcon::Standard(ChromeIcon::Error))
        );
        assert_eq!(chosen(&["error", "info"]), chosen(&["error"]));
        assert_ne!(chosen(&["error", "info"]), chosen(&["info"]));
    }

    #[test]
    fn a_non_standard_name_falls_through_to_transmitted_data() {
        let data = Notification99Data {
            icon_names: vec!["weather-clear".to_owned()],
            icon_data: Some(vec![1, 2, 3]),
            ..test_notification()
        };

        assert_eq!(
            untracked_icon_choice(&data),
            Some(UntrackedIcon::Transmitted(vec![1, 2, 3]))
        );
    }

    #[test]
    fn no_names_and_data_is_the_data() {
        let data = Notification99Data {
            icon_data: Some(vec![9, 8, 7]),
            ..test_notification()
        };

        assert_eq!(
            untracked_icon_choice(&data),
            Some(UntrackedIcon::Transmitted(vec![9, 8, 7]))
        );
    }

    #[test]
    fn a_non_standard_name_without_data_has_no_icon() {
        let data = Notification99Data {
            icon_names: vec!["weather-clear".to_owned()],
            ..test_notification()
        };

        assert_eq!(untracked_icon_choice(&data), None);
        assert_eq!(untracked_icon_choice(&test_notification()), None);
    }

    // ── the request ─────────────────────────────────────────────────────

    #[test]
    fn a_request_carries_the_notification_as_plain_data() {
        let data = Notification99Data {
            title: "T".to_owned(),
            body: Some("B".to_owned()),
            sound: Osc99Sound::Silent,
            expiry: Osc99Expiry::Never,
            icon_names: vec!["warn".to_owned()],
            buttons: vec!["Yes".to_owned()],
            app_name: Some("app".to_owned()),
            ..test_notification()
        };

        assert_eq!(
            DeliveryRequest::from_notification(&data),
            DeliveryRequest {
                title: "T".to_owned(),
                body: Some("B".to_owned()),
                sound: Osc99PlatformSound::Silent,
                expiry: Osc99Expiry::Never,
                icon: Some(UntrackedIcon::Standard(ChromeIcon::Warning)),
            }
        );
    }

    #[test]
    fn the_body_stands_in_for_an_empty_title_and_is_not_repeated() {
        let with_body = |title: &str, body: Option<&str>| DeliveryRequest {
            body: body.map(str::to_owned),
            ..request(title)
        };

        assert_eq!(
            with_body("Title", Some("Body")).lines(),
            ("Title", Some("Body"))
        );
        assert_eq!(with_body("Title", None).lines(), ("Title", None));
        assert_eq!(with_body("", Some("Body")).lines(), ("Body", None));
        assert_eq!(with_body("", None).lines(), ("", None));
    }

    // ── the queue ───────────────────────────────────────────────────────

    #[test]
    fn a_full_queue_drops_the_request_and_a_dead_one_reports_it() {
        let (queue, requests) = crossbeam_channel::bounded(QUEUE_CAPACITY);

        for n in 0..QUEUE_CAPACITY {
            assert_eq!(
                try_enqueue(&queue, request(&format!("n{n}"))),
                Submit::Queued
            );
        }
        assert_eq!(
            try_enqueue(&queue, request("overflow")),
            Submit::DroppedFull
        );
        assert_eq!(requests.len(), QUEUE_CAPACITY);
        // The oldest request is the one that is still first: the new one was
        // the one dropped.
        assert_eq!(requests.recv().unwrap().title, "n0");

        // Room again.
        assert_eq!(try_enqueue(&queue, request("again")), Submit::Queued);

        drop(requests);
        assert_eq!(try_enqueue(&queue, request("late")), Submit::WorkerGone);
    }

    #[test]
    fn the_queue_holds_sixteen() {
        assert_eq!(QUEUE_CAPACITY, 16);
    }

    #[test]
    fn a_worker_that_is_busy_drops_what_does_not_fit() {
        let (_root, store) = private_store();
        let (started_tx, started_rx) = crossbeam_channel::unbounded::<()>();
        let (release_tx, release_rx) = crossbeam_channel::unbounded::<()>();
        let (shown_tx, shown_rx) = crossbeam_channel::unbounded::<String>();
        let mut worker = DeliveryWorker::spawn(store, move |request, _| {
            let _ = started_tx.send(());
            // Held here until the test lets go (or drops the sender).
            let _ = release_rx.recv();
            let _ = shown_tx.send(request.title.clone());
        })
        .unwrap();

        // The first request is taken by the worker, which then blocks.
        assert_eq!(worker.submit(request("first")), Submit::Queued);
        started_rx.recv().unwrap();
        // The queue is now empty and the worker is busy: 16 fit, the 17th
        // does not.
        for n in 0..QUEUE_CAPACITY {
            assert_eq!(
                worker.submit(request(&format!("queued-{n}"))),
                Submit::Queued
            );
        }
        assert_eq!(worker.submit(request("dropped")), Submit::DroppedFull);

        drop(release_tx);
        worker.shutdown(Duration::from_secs(30));

        let shown: Vec<String> = shown_rx.try_iter().collect();
        assert_eq!(shown.len(), 1 + QUEUE_CAPACITY);
        assert_eq!(shown[0], "first");
        assert_eq!(shown[QUEUE_CAPACITY], "queued-15");
        assert!(!shown.iter().any(|title| title == "dropped"));
    }

    // ── the worker ──────────────────────────────────────────────────────

    #[test]
    fn the_worker_shows_requests_in_order_on_its_own_named_thread() {
        let (_root, store) = private_store();
        let (shown_tx, shown_rx) = crossbeam_channel::unbounded();
        let mut worker = DeliveryWorker::spawn(store, move |request, image| {
            let name = std::thread::current().name().map(str::to_owned);
            let _ = shown_tx.send((request.title.clone(), image.is_some(), name));
        })
        .unwrap();

        assert_eq!(worker.submit(request("one")), Submit::Queued);
        assert_eq!(worker.submit(request("two")), Submit::Queued);
        worker.shutdown(Duration::from_secs(30));

        let shown: Vec<_> = shown_rx.try_iter().collect();
        let name = Some("freminal-osc99-deliver".to_owned());
        assert_eq!(
            shown,
            [
                ("one".to_owned(), false, name.clone()),
                ("two".to_owned(), false, name)
            ]
        );
    }

    #[test]
    fn the_worker_hands_the_shower_a_png_file_for_the_icon_and_cleans_up_on_exit() {
        let (root, store) = private_store();
        let dir = root.path().join("icons");
        let (shown_tx, shown_rx) = crossbeam_channel::unbounded();
        let mut worker = DeliveryWorker::spawn(store, move |_, image| {
            // The file exists while the notification is being shown.
            let _ = shown_tx.send(image.map(|path| (path.to_path_buf(), path.is_file())));
        })
        .unwrap();

        let standard = DeliveryRequest {
            icon: Some(UntrackedIcon::Standard(ChromeIcon::Error)),
            ..request("standard")
        };
        let transmitted = DeliveryRequest {
            icon: Some(UntrackedIcon::Transmitted(png(8, 8))),
            ..request("transmitted")
        };
        assert_eq!(worker.submit(standard), Submit::Queued);
        assert_eq!(worker.submit(transmitted), Submit::Queued);
        worker.shutdown(Duration::from_secs(30));

        let shown: Vec<_> = shown_rx.try_iter().collect();
        assert_eq!(shown.len(), 2);
        for entry in &shown {
            let (path, existed) = entry.as_ref().unwrap();
            assert!(existed, "{path:?} existed during show");
            assert!(path.starts_with(&dir));
            assert_eq!(path.extension().unwrap(), "png");
        }
        assert!(
            !dir.exists(),
            "the worker removed its files and the directory on exit"
        );
    }

    #[test]
    fn dropping_the_worker_stops_it_and_removes_the_files() {
        let (root, store) = private_store();
        let dir = root.path().join("icons");
        let worker = DeliveryWorker::spawn(store, |_, _| {}).unwrap();
        let transmitted = DeliveryRequest {
            icon: Some(UntrackedIcon::Transmitted(png(4, 4))),
            ..request("t")
        };
        assert_eq!(worker.submit(transmitted), Submit::Queued);

        drop(worker);

        assert!(!dir.exists());
    }

    #[test]
    fn a_stopped_worker_reports_itself_gone() {
        let (_root, store) = private_store();
        let mut worker = DeliveryWorker::spawn(store, |_, _| {}).unwrap();

        worker.shutdown(Duration::from_secs(30));

        assert_eq!(worker.submit(request("late")), Submit::WorkerGone);
    }

    #[test]
    fn a_panicking_shower_makes_the_worker_report_gone() {
        let (_root, store) = private_store();
        let mut worker = DeliveryWorker::spawn(store, |_, _| panic!("show failed")).unwrap();
        assert_eq!(worker.submit(request("boom")), Submit::Queued);
        // The thread unwinds and drops its receiver; wait for that, then
        // check the sender notices.
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut result = Submit::Queued;
        while result != Submit::WorkerGone && Instant::now() < deadline {
            result = worker.submit(request("after"));
            std::thread::sleep(Duration::from_millis(2));
        }

        assert_eq!(result, Submit::WorkerGone);
        worker.shutdown(Duration::from_secs(30));
    }

    // ── the icon preparer ───────────────────────────────────────────────

    #[test]
    fn a_standard_icon_is_drawn_once_and_reuses_one_file() {
        let (_root, store) = private_store();
        let mut preparer = IconPreparer::new(store);
        let icon = UntrackedIcon::Standard(ChromeIcon::Info);

        let first = preparer.image_path(Some(&icon)).unwrap();
        let drawn_at = preparer.drawn[&ChromeIcon::Info].as_ref().unwrap().as_ptr();
        let second = preparer.image_path(Some(&icon)).unwrap();

        assert_eq!(first, second);
        assert_eq!(decoded_size(&first), (256, 256));
        assert_eq!(preparer.drawn.len(), 1);
        assert_eq!(
            preparer.drawn[&ChromeIcon::Info].as_ref().unwrap().as_ptr(),
            drawn_at,
            "the cached PNG was not drawn again"
        );

        let other = preparer
            .image_path(Some(&UntrackedIcon::Standard(ChromeIcon::Error)))
            .unwrap();
        assert_ne!(first, other);
        assert_eq!(preparer.drawn.len(), 2);
    }

    #[test]
    fn a_transmitted_icon_is_reencoded_and_bounded() {
        let (_root, store) = private_store();
        let mut preparer = IconPreparer::new(store);

        let path = preparer
            .image_path(Some(&UntrackedIcon::Transmitted(png(1000, 500))))
            .unwrap();

        assert_eq!(decoded_size(&path), (256, 128));
        assert_eq!(path.extension().unwrap(), "png");
    }

    #[test]
    fn what_the_os_reads_is_never_the_transmitted_bytes() {
        let (_root, store) = private_store();
        let mut preparer = IconPreparer::new(store);
        // A valid image with trailing junk (a polyglot): the file written is
        // the clean re-encode, not the input.
        let mut polyglot = png(4, 4);
        polyglot.extend_from_slice(b"<script>junk after IEND</script>");

        let path = preparer
            .image_path(Some(&UntrackedIcon::Transmitted(polyglot.clone())))
            .unwrap();

        let written = std::fs::read(path).unwrap();
        assert_ne!(written, polyglot);
        assert!(!written.windows(8).any(|window| window == b"<script>"));
    }

    #[test]
    fn undecodable_and_oversized_transmitted_bytes_give_no_image() {
        let (root, store) = private_store();
        let mut preparer = IconPreparer::new(store);

        for bytes in [b"not an image".to_vec(), Vec::new(), png(4097, 1)] {
            assert_eq!(
                preparer.image_path(Some(&UntrackedIcon::Transmitted(bytes))),
                None
            );
        }
        assert!(
            !root.path().join("icons").exists(),
            "nothing was written for them"
        );
    }

    #[test]
    fn no_icon_means_no_image() {
        let (_root, store) = private_store();
        let mut preparer = IconPreparer::new(store);

        assert_eq!(preparer.image_path(None), None);
    }

    #[test]
    fn an_unsafe_directory_means_no_image_not_a_failure() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("icons");
        std::fs::write(&dir, b"not a directory").unwrap();
        let mut preparer = IconPreparer::new(IconTempFiles::with_dir(dir));

        assert_eq!(
            preparer.image_path(Some(&UntrackedIcon::Standard(ChromeIcon::Info))),
            None
        );
    }

    // ── macOS notification ──────────────────────────────────────────────

    #[test]
    fn the_macos_notification_maps_expiry_to_the_timeout() {
        let timeout_for = |expiry| {
            build_macos_notification(
                &DeliveryRequest {
                    expiry,
                    ..request("t")
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
            timeout_for(Osc99Expiry::After(NonZeroU32::new(5000).unwrap())),
            notify_rust::Timeout::Milliseconds(5000)
        );
    }

    #[test]
    fn the_macos_notification_has_the_headline_the_detail_and_no_buttons() {
        let titled = build_macos_notification(
            &DeliveryRequest {
                body: Some("Body".to_owned()),
                ..request("Title")
            },
            None,
        );
        assert_eq!(titled.summary, "Title");
        assert_eq!(titled.body, "Body");
        assert_eq!(titled.actions, Vec::<String>::new());

        let untitled = build_macos_notification(
            &DeliveryRequest {
                body: Some("Body".to_owned()),
                ..request("")
            },
            None,
        );
        assert_eq!(untitled.summary, "Body");
        assert_eq!(untitled.body, "");
    }

    // ── Windows toast ───────────────────────────────────────────────────

    #[test]
    fn the_toast_is_long_for_never_and_for_25_seconds_or_more() {
        let after = |ms| Osc99Expiry::After(NonZeroU32::new(ms).unwrap());

        assert_eq!(toast_length(Osc99Expiry::OsDefault), ToastLength::Short);
        assert_eq!(toast_length(after(1)), ToastLength::Short);
        assert_eq!(toast_length(after(24_999)), ToastLength::Short);
        assert_eq!(toast_length(after(25_000)), ToastLength::Long);
        assert_eq!(toast_length(after(u32::MAX)), ToastLength::Long);
        assert_eq!(toast_length(Osc99Expiry::Never), ToastLength::Long);
    }

    #[test]
    fn a_plain_windows_temp_path_becomes_a_forward_slash_path() {
        let path = Path::new(r"C:\Users\fred\AppData\Local\Temp\freminal-osc99-icons-12\ab.png");

        assert_eq!(
            toast_file_path(path),
            Some(PathBuf::from(
                "C:/Users/fred/AppData/Local/Temp/freminal-osc99-icons-12/ab.png"
            ))
        );
    }

    #[test]
    fn spaces_percent_and_non_ascii_in_the_path_are_percent_encoded() {
        let path = Path::new(r"C:\Users\John Smith\100%\a#b?c\icon.png");
        let spaced = toast_file_path(Path::new(r"D:\Users\John Smith\100%\icon.png"));

        assert_eq!(
            spaced,
            Some(PathBuf::from("D:/Users/John%20Smith/100%25/icon.png"))
        );
        // Every other byte outside the unreserved set is encoded too.
        let encoded = toast_file_path(path).unwrap();
        let text = encoded.to_str().unwrap();
        assert!(!text.contains(' ') && !text.contains('#') && !text.contains('?'));
        assert!(text.contains("%23") && text.contains("%3F"));

        let accented = toast_file_path(Path::new("C:\\ol\u{e9}\\i.png")).unwrap();
        assert_eq!(accented.to_str().unwrap(), "C:/ol%C3%A9/i.png");
    }

    #[test]
    fn a_path_that_is_not_drive_rooted_has_no_toast_path() {
        for path in [
            r"\\server\share\icon.png",
            r"\\?\C:\Temp\icon.png",
            r"relative\icon.png",
            "/tmp/icon.png",
            r"C:icon.png",
            "",
        ] {
            assert_eq!(toast_file_path(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn a_drive_path_with_forward_slashes_is_accepted() {
        assert_eq!(
            toast_file_path(Path::new("c:/Temp/icon.png")),
            Some(PathBuf::from("c:/Temp/icon.png"))
        );
    }
}
