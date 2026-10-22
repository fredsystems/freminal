//! The macOS and Windows delivery worker for freminal's own notifications.
//!
//! Showing a notification can block, so it never runs on the egui frame thread:
//! [`show_plain`] hands a [`PlainNotification`](super::PlainNotification) to one
//! `freminal-notify` thread through a bounded queue, and starts a replacement
//! (rate-limited) if that thread has exited. Linux and BSD have no such worker;
//! their notifications go through the D-Bus service. Neither platform's
//! presenter reads the notification's urgency.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
#[cfg(not(all(unix, not(target_os = "macos"))))]
use std::{sync::OnceLock, time::Instant};

use arc_swap::ArcSwapOption;
use crossbeam_channel::Sender;

#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
use super::plain_sink::PlainNotification;
use super::queue::{Offered, offer};

/// What freminal asks macOS to show for one plain notification: everything
/// [`macos_plain_fields`] decides, with no dependence on the presenter.
#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct MacosPlainFields {
    /// The application name.
    appname: &'static str,
    /// The icon-theme name of the application icon.
    icon: &'static str,
    /// The notification's title.
    summary: String,
    /// The notification's text.
    body: String,
}

/// Decide the macOS notification content for `notification`: the freminal
/// application icon by icon-theme **name** (`"freminal"`, matching
/// `assets/freminal.desktop`'s `Icon=freminal`), no urgency (macOS has none).
///
/// Pure and compiled in test builds on every platform; the conversion to the
/// presenter's type is `macos_notification`.
#[cfg(any(target_os = "macos", test))]
fn macos_plain_fields(notification: &PlainNotification) -> MacosPlainFields {
    MacosPlainFields {
        appname: "freminal",
        icon: "freminal",
        summary: notification.summary.clone(),
        body: notification.body.clone(),
    }
}

/// The macOS notification for `notification`: [`macos_plain_fields`], copied
/// into the presenter's type.
#[cfg(target_os = "macos")]
fn macos_notification(notification: &PlainNotification) -> notify_rust::Notification {
    let fields = macos_plain_fields(notification);
    let mut built = notify_rust::Notification::new();
    built
        .appname(fields.appname)
        .icon(fields.icon)
        .summary(&fields.summary)
        .body(&fields.body);
    built
}

/// The application id a Windows toast is attributed to.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToastAppId {
    /// The PowerShell app id: freminal has no `AppUserModelID` of its own.
    PowerShell,
}

/// The sound a plain Windows toast plays.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlainToastSound {
    /// No sound.
    Silent,
}

/// How long a plain Windows toast stays up.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlainToastDuration {
    /// The short toast (about 7 s).
    Short,
}

/// What freminal asks Windows to show for one plain notification: everything
/// [`windows_plain_toast_fields`] decides, with no dependence on the
/// presenter.
#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
struct WindowsPlainToastFields {
    /// The application id the toast is attributed to.
    app_id: ToastAppId,
    /// The toast's title.
    title: String,
    /// The text line; `None` for an empty body.
    text1: Option<String>,
    /// The sound to play.
    sound: PlainToastSound,
    /// How long the toast stays up.
    duration: PlainToastDuration,
}

/// Decide the Windows toast content for `notification`: the summary as the
/// title and the body as the text, silent, short, attributed to the
/// PowerShell app id (freminal has no `AppUserModelID` of its own).
///
/// There is no scenario. A "reminder" toast needs buttons (Task 138.C15), and
/// urgency is not advertised on Windows (138.C17), so an error notification is
/// an ordinary toast. An empty body adds no text line.
///
/// Pure and compiled in test builds on every platform; the conversion to the
/// presenter's type is `windows_toast`.
#[cfg(any(target_os = "windows", test))]
fn windows_plain_toast_fields(notification: &PlainNotification) -> WindowsPlainToastFields {
    WindowsPlainToastFields {
        app_id: ToastAppId::PowerShell,
        title: notification.summary.clone(),
        text1: (!notification.body.is_empty()).then(|| notification.body.clone()),
        sound: PlainToastSound::Silent,
        duration: PlainToastDuration::Short,
    }
}

/// The Windows toast for `notification`: [`windows_plain_toast_fields`],
/// copied into a `tauri-winrt-notification` toast, built directly in the same
/// style as the OSC 99 toast (`desktop_notify::untracked`).
#[cfg(target_os = "windows")]
fn windows_toast(notification: &PlainNotification) -> tauri_winrt_notification::Toast {
    use tauri_winrt_notification::{Duration as ToastDuration, Toast};

    let fields = windows_plain_toast_fields(notification);
    let app_id = match fields.app_id {
        ToastAppId::PowerShell => Toast::POWERSHELL_APP_ID,
    };
    let mut toast = Toast::new(app_id).title(&fields.title);
    if let Some(text1) = &fields.text1 {
        toast = toast.text1(text1);
    }
    toast
        .sound(match fields.sound {
            PlainToastSound::Silent => None,
        })
        .duration(match fields.duration {
            PlainToastDuration::Short => ToastDuration::Short,
        })
}

/// Show `notification` on this platform, logging a failure.
///
/// Called only from the worker thread, which only `PlainSink::platform_worker`
/// feeds: never from a test.
#[cfg(target_os = "windows")]
fn show_platform_notification(notification: &PlainNotification) {
    if let Err(e) = windows_toast(notification).show() {
        tracing::warn!("failed to show desktop notification: {e}");
    }
}

/// Show `notification` on this platform, logging a failure.
///
/// Called only from the worker thread, which only `PlainSink::platform_worker`
/// feeds: never from a test.
#[cfg(target_os = "macos")]
fn show_platform_notification(notification: &PlainNotification) {
    if let Err(e) = macos_notification(notification).show() {
        tracing::warn!("failed to show desktop notification: {e}");
    }
}

/// How many desktop notifications may wait for the notify worker. A program
/// that emits OSC 9 / OSC 777 in a loop must not be able to grow an unbounded
/// backlog (or an unbounded number of threads); past this, notifications are
/// dropped.
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
const SYSTEM_NOTIFICATION_QUEUE: usize = 16;

/// What [`enqueue_bounded`] did with an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
enum EnqueueOutcome {
    /// The item is waiting for the worker.
    Enqueued,
    /// The queue was full; the item was dropped.
    DroppedFull,
    /// The worker has exited and could not be replaced; the item was dropped.
    WorkerGone,
    /// The worker has exited, and a replacement was started too recently to
    /// start another (see [`NOTIFY_WORKER_RESPAWN_MIN_INTERVAL`]); the item
    /// was dropped.
    RespawnSuppressed,
}

/// Hand `item` to a bounded queue without ever blocking or spawning; a
/// disconnected queue hands the item back as `Err` so the caller can retry it
/// on a replacement worker.
///
/// This is the whole queueing policy of the desktop-notification path, kept
/// separate from the OS call so it is testable without one.
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
fn enqueue_bounded<T>(queue: &Sender<T>, item: T) -> Result<EnqueueOutcome, T> {
    match offer(queue, item)? {
        Offered::Queued => Ok(EnqueueOutcome::Enqueued),
        Offered::Full => Ok(EnqueueOutcome::DroppedFull),
    }
}

/// The minimum time between two starts of the notification worker. A worker
/// that panics on every notification must not turn into a thread spawned per
/// notification.
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
const NOTIFY_WORKER_RESPAWN_MIN_INTERVAL: Duration = Duration::from_secs(5);

/// Whether a dead (or absent) notification worker may be started again now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
enum RespawnDecision {
    /// Enough time has passed since the last start (or there was none).
    Respawn,
    /// The last start was too recent.
    TooSoon,
}

/// Decide whether a worker may be started, given the time since the previous
/// start (`None` if there has never been one).
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
fn respawn_decision(since_last_spawn: Option<Duration>) -> RespawnDecision {
    match since_last_spawn {
        Some(elapsed) if elapsed < NOTIFY_WORKER_RESPAWN_MIN_INTERVAL => RespawnDecision::TooSoon,
        Some(_) | None => RespawnDecision::Respawn,
    }
}

/// Sentinel for "the worker has never been started".
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
const NEVER_SPAWNED: u64 = u64::MAX;

/// Atomically claim the right to start a worker at `now_ms` (milliseconds
/// since process start), recording it in `last_spawn_ms` when granted. Of
/// several concurrent callers, at most one is granted.
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
fn claim_respawn(last_spawn_ms: &AtomicU64, now_ms: u64) -> RespawnDecision {
    let last = last_spawn_ms.load(Ordering::Acquire);
    let since_last =
        (last != NEVER_SPAWNED).then(|| Duration::from_millis(now_ms.saturating_sub(last)));
    match respawn_decision(since_last) {
        RespawnDecision::TooSoon => RespawnDecision::TooSoon,
        RespawnDecision::Respawn => {
            match last_spawn_ms.compare_exchange(
                last,
                // `NEVER_SPAWNED` is reserved; clamp so a start is never
                // mistaken for "never" (unreachable in practice: ~584M years).
                now_ms.min(NEVER_SPAWNED - 1),
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => RespawnDecision::Respawn,
                Err(_) => RespawnDecision::TooSoon,
            }
        }
    }
}

/// [`claim_respawn`] against the process-wide start record and clock.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn claim_notify_worker_respawn() -> RespawnDecision {
    static START: OnceLock<Instant> = OnceLock::new();
    static LAST_SPAWN_MS: AtomicU64 = AtomicU64::new(NEVER_SPAWNED);
    let now_ms = u64::try_from(START.get_or_init(Instant::now).elapsed().as_millis())
        .unwrap_or(NEVER_SPAWNED - 1);
    claim_respawn(&LAST_SPAWN_MS, now_ms)
}

/// Enqueue `item` on the worker in `slot`, replacing the worker if it has
/// exited (or was never started).
///
/// `may_respawn` is consulted only when a replacement is needed; it is the
/// rate limit. `spawn` starts a fresh worker and returns its queue. The
/// replacement is installed with a compare-and-swap: if another caller
/// installed one first, theirs is used and ours is dropped (its worker exits
/// when the sender does). The enqueue is retried once on the new queue.
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
fn enqueue_with_respawn<T>(
    slot: &ArcSwapOption<Sender<T>>,
    item: T,
    may_respawn: impl FnOnce() -> RespawnDecision,
    spawn: impl FnOnce() -> Option<Sender<T>>,
) -> EnqueueOutcome {
    let current = slot.load_full();
    let item = match &current {
        Some(queue) => match enqueue_bounded(queue, item) {
            Ok(outcome) => return outcome,
            Err(item) => item,
        },
        None => item,
    };

    if may_respawn() == RespawnDecision::TooSoon {
        return EnqueueOutcome::RespawnSuppressed;
    }
    let Some(fresh) = spawn().map(std::sync::Arc::new) else {
        return EnqueueOutcome::WorkerGone;
    };
    let previous = slot.compare_and_swap(&current, Some(std::sync::Arc::clone(&fresh)));
    let installed = match &*previous {
        // Our swap lost: use the winner's queue and drop `fresh`.
        winner if !same_queue(winner.as_ref(), current.as_ref()) => winner.clone().unwrap_or(fresh),
        _ => fresh,
    };
    match enqueue_bounded(&installed, item) {
        Ok(outcome) => outcome,
        Err(_item) => EnqueueOutcome::WorkerGone,
    }
}

/// Whether two optional queue handles are the same allocation.
#[cfg(any(test, not(all(unix, not(target_os = "macos")))))]
fn same_queue<T>(a: Option<&std::sync::Arc<T>>, b: Option<&std::sync::Arc<T>>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
        (None, None) => true,
        _ => false,
    }
}

/// Log a notification dropped because the queue was full: a warning for the
/// first, then debug, so a flood cannot also flood the log.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn log_dropped_notification() {
    static DROPPED: AtomicU64 = AtomicU64::new(0);
    if DROPPED.fetch_add(1, Ordering::Relaxed) == 0 {
        tracing::warn!(
            "desktop notification queue is full; dropping notifications \
             (further drops are logged at debug level)"
        );
    } else {
        tracing::debug!("desktop notification queue is full; notification dropped");
    }
}

/// The queue feeding the one long-lived `freminal-notify` worker. Empty until
/// first use, and replaced (rate-limited) if the worker exits; see
/// [`enqueue_with_respawn`].
#[cfg(not(all(unix, not(target_os = "macos"))))]
static NOTIFY_WORKER: ArcSwapOption<Sender<PlainNotification>> = ArcSwapOption::const_empty();

/// Start a `freminal-notify` worker and return its bounded queue. `None` if
/// the worker thread could not be spawned.
#[cfg(not(all(unix, not(target_os = "macos"))))]
fn spawn_notify_worker() -> Option<Sender<PlainNotification>> {
    let (tx, rx) = crossbeam_channel::bounded::<PlainNotification>(SYSTEM_NOTIFICATION_QUEUE);
    let spawned = std::thread::Builder::new()
        .name("freminal-notify".to_owned())
        .spawn(move || {
            for notification in rx {
                show_platform_notification(&notification);
            }
        });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            tracing::warn!("failed to spawn notification thread: {e}");
            None
        }
    }
}

/// Show `notification` on the long-lived `freminal-notify` worker (macOS and
/// Windows).
///
/// Showing a notification can block, so it must not run on the egui frame
/// thread. Failures are logged and otherwise ignored -- a missing notification
/// daemon is non-fatal.
///
/// Notifications go to one long-lived worker through a bounded queue
/// ([`SYSTEM_NOTIFICATION_QUEUE`]); when it is full the notification is
/// dropped, never blocking the frame loop and never spawning a thread. The
/// worker is a process-wide queue. The queueing policy is covered separately
/// through [`enqueue_bounded`].
#[cfg(not(all(unix, not(target_os = "macos"))))]
pub fn show_plain(notification: PlainNotification) {
    match enqueue_with_respawn(
        &NOTIFY_WORKER,
        notification,
        claim_notify_worker_respawn,
        spawn_notify_worker,
    ) {
        EnqueueOutcome::Enqueued => {}
        EnqueueOutcome::DroppedFull => log_dropped_notification(),
        EnqueueOutcome::WorkerGone => {
            tracing::warn!(
                "desktop notification worker could not be (re)started; notification dropped"
            );
        }
        EnqueueOutcome::RespawnSuppressed => {
            tracing::debug!(
                "desktop notification worker exited and was restarted recently; \
                 notification dropped"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::buffer_states::osc99_notification::NotificationUrgency;

    /// The queueing policy never blocks and never spawns: against a queue
    /// nobody drains, exactly its capacity is accepted and the rest dropped.
    #[test]
    fn enqueue_bounded_accepts_capacity_then_drops() {
        let (tx, _rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        let mut enqueued = 0_usize;
        let mut dropped = 0_usize;
        for i in 0..1000_u32 {
            match enqueue_bounded(&tx, i) {
                Ok(EnqueueOutcome::Enqueued) => enqueued += 1,
                Ok(EnqueueOutcome::DroppedFull) => dropped += 1,
                Ok(EnqueueOutcome::WorkerGone | EnqueueOutcome::RespawnSuppressed) | Err(_) => {
                    unreachable!("receiver is alive")
                }
            }
        }
        assert_eq!(enqueued, SYSTEM_NOTIFICATION_QUEUE);
        assert_eq!(dropped, 1000 - SYSTEM_NOTIFICATION_QUEUE);
    }

    #[test]
    fn enqueue_bounded_reports_a_gone_worker() {
        let (tx, rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        drop(rx);
        assert_eq!(enqueue_bounded(&tx, 1), Err(1));
    }

    #[test]
    fn respawn_decision_honours_the_minimum_interval() {
        assert_eq!(respawn_decision(None), RespawnDecision::Respawn);
        assert_eq!(
            respawn_decision(Some(Duration::ZERO)),
            RespawnDecision::TooSoon
        );
        assert_eq!(
            respawn_decision(Some(Duration::from_millis(4_999))),
            RespawnDecision::TooSoon
        );
        assert_eq!(
            respawn_decision(Some(NOTIFY_WORKER_RESPAWN_MIN_INTERVAL)),
            RespawnDecision::Respawn
        );
    }

    #[test]
    fn claim_respawn_records_the_start_and_rate_limits() {
        let last = AtomicU64::new(NEVER_SPAWNED);
        assert_eq!(claim_respawn(&last, 100), RespawnDecision::Respawn);
        assert_eq!(claim_respawn(&last, 200), RespawnDecision::TooSoon);
        let interval_ms = u64::try_from(NOTIFY_WORKER_RESPAWN_MIN_INTERVAL.as_millis()).unwrap();
        assert_eq!(
            claim_respawn(&last, 100 + interval_ms - 1),
            RespawnDecision::TooSoon
        );
        assert_eq!(
            claim_respawn(&last, 100 + interval_ms),
            RespawnDecision::Respawn
        );
    }

    fn live_slot() -> (ArcSwapOption<Sender<u32>>, crossbeam_channel::Receiver<u32>) {
        let (tx, rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        (ArcSwapOption::from_pointee(tx), rx)
    }

    #[test]
    fn live_worker_is_used_without_consulting_the_respawn_hooks() {
        let (slot, rx) = live_slot();
        let outcome = enqueue_with_respawn(
            &slot,
            7,
            || unreachable!("worker is alive"),
            || unreachable!("worker is alive"),
        );
        assert_eq!(outcome, EnqueueOutcome::Enqueued);
        assert_eq!(rx.try_recv(), Ok(7));
    }

    #[test]
    fn dead_worker_is_replaced_and_the_item_retried() {
        let (slot, dead_rx) = live_slot();
        drop(dead_rx);
        let (fresh_tx, fresh_rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        let outcome =
            enqueue_with_respawn(&slot, 9, || RespawnDecision::Respawn, || Some(fresh_tx));
        assert_eq!(outcome, EnqueueOutcome::Enqueued);
        assert_eq!(fresh_rx.try_recv(), Ok(9));

        // The replacement is now the installed worker.
        assert_eq!(
            enqueue_with_respawn(
                &slot,
                10,
                || unreachable!("replacement is alive"),
                || unreachable!("replacement is alive"),
            ),
            EnqueueOutcome::Enqueued
        );
        assert_eq!(fresh_rx.try_recv(), Ok(10));
    }

    #[test]
    fn empty_slot_starts_a_worker() {
        let slot = ArcSwapOption::<Sender<u32>>::empty();
        let (tx, rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        let outcome = enqueue_with_respawn(&slot, 1, || RespawnDecision::Respawn, || Some(tx));
        assert_eq!(outcome, EnqueueOutcome::Enqueued);
        assert_eq!(rx.try_recv(), Ok(1));
        assert!(slot.load().is_some());
    }

    #[test]
    fn too_soon_drops_the_item_without_spawning() {
        let (slot, dead_rx) = live_slot();
        drop(dead_rx);
        let outcome = enqueue_with_respawn(
            &slot,
            1,
            || RespawnDecision::TooSoon,
            || unreachable!("rate limited"),
        );
        assert_eq!(outcome, EnqueueOutcome::RespawnSuppressed);
    }

    #[test]
    fn failed_spawn_reports_a_gone_worker() {
        let slot = ArcSwapOption::<Sender<u32>>::empty();
        let outcome = enqueue_with_respawn(&slot, 1, || RespawnDecision::Respawn, || None);
        assert_eq!(outcome, EnqueueOutcome::WorkerGone);
        assert!(slot.load().is_none());
    }

    /// If another caller installs a replacement while ours is being started,
    /// theirs wins and receives the item; ours is dropped.
    #[test]
    fn losing_the_swap_race_uses_the_winners_queue() {
        let (slot, dead_rx) = live_slot();
        drop(dead_rx);
        let (winner_tx, winner_rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        let (loser_tx, loser_rx) = crossbeam_channel::bounded::<u32>(SYSTEM_NOTIFICATION_QUEUE);
        let outcome = enqueue_with_respawn(
            &slot,
            5,
            || RespawnDecision::Respawn,
            || {
                // The concurrent winner installs its queue before we do.
                slot.store(Some(std::sync::Arc::new(winner_tx)));
                Some(loser_tx)
            },
        );
        assert_eq!(outcome, EnqueueOutcome::Enqueued);
        assert_eq!(winner_rx.try_recv(), Ok(5));
        assert!(loser_rx.try_recv().is_err());
    }

    fn plain(summary: &str, body: &str) -> PlainNotification {
        PlainNotification {
            summary: summary.to_owned(),
            body: body.to_owned(),
            urgency: NotificationUrgency::Normal,
        }
    }

    /// Slice 3 (#433): on macOS the worker attaches the bundled freminal
    /// application icon by name so desktop notifications show the app icon.
    /// The name matches `assets/freminal.desktop`'s `Icon=freminal`. (On
    /// Linux/BSD the D-Bus service sends the same name, plus the icon itself
    /// as `image-data`; see the notification service's `plain` module.)
    #[test]
    fn macos_plain_fields_set_the_freminal_icon() {
        let fields = macos_plain_fields(&plain("Hello", "world"));
        assert_eq!(fields.icon, "freminal");
        assert_eq!(fields.appname, "freminal");
        assert_eq!(fields.summary, "Hello");
        assert_eq!(fields.body, "world");
    }

    #[test]
    fn macos_plain_fields_copy_an_empty_body_as_empty() {
        let fields = macos_plain_fields(&plain("Hello", ""));
        assert_eq!(fields.summary, "Hello");
        assert_eq!(fields.body, "");
    }

    #[test]
    fn macos_plain_fields_ignore_the_urgency() {
        let critical = PlainNotification {
            urgency: NotificationUrgency::Critical,
            ..plain("Hello", "world")
        };
        assert_eq!(
            macos_plain_fields(&critical),
            macos_plain_fields(&plain("Hello", "world"))
        );
    }

    #[test]
    fn windows_plain_toast_fields_copy_the_summary_and_body() {
        let fields = windows_plain_toast_fields(&plain("Hello", "world"));
        assert_eq!(fields.title, "Hello");
        assert_eq!(fields.text1.as_deref(), Some("world"));
    }

    #[test]
    fn windows_plain_toast_fields_leave_out_text1_for_an_empty_body() {
        let fields = windows_plain_toast_fields(&plain("Hello", ""));
        assert_eq!(fields.title, "Hello");
        assert_eq!(fields.text1, None);
    }

    #[test]
    fn windows_plain_toast_fields_are_silent_short_and_powershell() {
        let fields = windows_plain_toast_fields(&plain("Hello", "world"));
        assert_eq!(fields.sound, PlainToastSound::Silent);
        assert_eq!(fields.duration, PlainToastDuration::Short);
        assert_eq!(fields.app_id, ToastAppId::PowerShell);
    }

    #[test]
    fn windows_plain_toast_fields_ignore_the_urgency() {
        let critical = PlainNotification {
            urgency: NotificationUrgency::Critical,
            ..plain("Hello", "world")
        };
        assert_eq!(
            windows_plain_toast_fields(&critical),
            windows_plain_toast_fields(&plain("Hello", "world"))
        );
    }
}
