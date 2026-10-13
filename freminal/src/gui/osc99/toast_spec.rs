//! What an OSC 99 notification looks like as a toast.

use std::time::Duration;

use freminal_common::buffer_states::osc99_notification::{
    Notification99Data, NotificationUrgency, Osc99Expiry,
};

use crate::gui::icons::{ChromeIcon, standard_icon_glyph};
use crate::gui::toast::{ToastKind, ToastLifetime};

/// How long a `Low`-urgency notification's toast stays when the application
/// did not ask for a duration (`w=-1`, or no `w=`): half the `Info` toast
/// default, so it follows that default if it changes.
pub const OSC99_LOW_URGENCY_TOAST_LIFETIME: Duration =
    match ToastKind::Info.default_duration().checked_div(2) {
        Some(half) => half,
        // Dividing by two cannot fail; the full default is the safe fallback.
        None => ToastKind::Info.default_duration(),
    };

/// What an OSC 99 notification looks like as a toast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Osc99ToastSpec {
    /// The toast severity.
    pub kind: ToastKind,
    /// The notification title.
    pub title: String,
    /// The notification body, if any.
    pub detail: Option<String>,
    /// How long the toast stays.
    pub lifetime: ToastLifetime,
    /// The button labels (`p=buttons`), in order. The toast keeps at most
    /// eight.
    pub buttons: Vec<String>,
    /// The glyph for the first `n=` name that is a standard icon name, names
    /// in order. `None` leaves the toast's kind icon.
    pub icon: Option<ChromeIcon>,
}

/// Build the toast for `data`.
///
/// - `Critical` urgency is an `Error` toast; everything else is `Info`.
/// - `w=` > 0 and `w=0` both stay until dismissed, as far as the toast is
///   concerned: the exact deadline of a positive `w=` is enforced by the
///   registry (`Osc99Registry::tick`, which closes the toast through
///   `CloseToast`), measured from registration. Giving the toast its own
///   `Fixed` lifetime as well would run a second, slightly different clock
///   (the toast's starts when it is first drawn, and hovering would extend
///   it), so hovering a `w=` toast would hold it past the deadline the
///   application asked for. Hovering therefore holds only toasts with a
///   toast-owned timer: the default and the `Low` lifetime below.
/// - `w=-1` (or unset) follows urgency: `Critical` stays until dismissed,
///   `Normal` uses the toast default, `Low` uses
///   [`OSC99_LOW_URGENCY_TOAST_LIFETIME`].
/// - The icon is the first standard `n=` name's glyph (see
///   [`standard_icon_glyph`]); toasts render no images, so transmitted icon
///   data never changes the toast.
#[must_use]
pub fn osc99_toast_spec(data: &Notification99Data) -> Osc99ToastSpec {
    let kind = match data.urgency {
        NotificationUrgency::Critical => ToastKind::Error,
        NotificationUrgency::Low | NotificationUrgency::Normal => ToastKind::Info,
    };
    let lifetime = match data.expiry {
        Osc99Expiry::After(_) | Osc99Expiry::Never => ToastLifetime::UntilDismissed,
        Osc99Expiry::OsDefault => match data.urgency {
            NotificationUrgency::Critical => ToastLifetime::UntilDismissed,
            NotificationUrgency::Normal => ToastLifetime::KindDefault,
            NotificationUrgency::Low => ToastLifetime::Fixed(OSC99_LOW_URGENCY_TOAST_LIFETIME),
        },
    };
    Osc99ToastSpec {
        kind,
        title: data.title.clone(),
        detail: data.body.clone().filter(|body| !body.is_empty()),
        lifetime,
        buttons: data.buttons.clone(),
        icon: data
            .icon_names
            .iter()
            .find_map(|name| standard_icon_glyph(name)),
    }
}
