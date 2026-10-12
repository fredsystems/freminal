// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! User-visible transient notifications.
//!
//! Toasts are short messages rendered as an overlay in the top-right corner
//! of every window.  They are used to surface non-fatal errors that used to
//! disappear into `tracing::error!` logs — PTY spawn failures, layout load
//! failures, shader compile errors, and similar.
//!
//! The stack lives at app-level on [`super::FreminalGui`] (not per-window) so
//! a failure that happens before a window exists (e.g. PTY spawn for a new
//! window) still has a place to be reported.  Every window renders the same
//! stack; a dismissal clears the toast for all windows at once.
//!
//! Toasts auto-expire after a kind-dependent duration unless the user hovers
//! over them, in which case the timer is paused.  Error toasts expire after
//! 15 seconds, warnings after 10, and info after 6.  The user can dismiss any
//! toast immediately by clicking anywhere on it; for an ordinary (unkeyed)
//! toast the "x" glyph shown at the pill's right edge is a visual affordance
//! for this, not a separate hit target.
//!
//! A toast may also carry an opaque [`ToastKey`] (see
//! [`ToastStack::push_keyed`]). Keyed toasts have a caller-chosen
//! [`ToastLifetime`] and report what happened to them as [`ToastEvent`]s
//! (drained with [`ToastStack::take_events`]): a click on the pill body is
//! `Activated` then `Closed`, a click on the "x" is `Closed`, and expiry,
//! eviction and [`ToastStack::close_keyed`] are `Closed`. Unkeyed toasts
//! never produce events.
//!
//! Toasts are drawn by a fully-owned OpenGL pass (issue #433), not egui
//! widgets: [`ToastStack::show`] measures/shapes each toast's text on the
//! main thread (`renderer::toast_text_pass::ToastTextRenderer`), lays it out
//! via [`layout_toasts`](layout::layout_toasts), and hands the resulting pill quads and text runs
//! to a single `PaintCallback` that draws pills
//! (`renderer::toast_pass::ToastRenderer`) then text on top. Hover/dismiss
//! hit-testing is done in owned code against egui's pointer state, since
//! there are no interactive egui widgets to generate `Response`s.

use std::time::{Duration, Instant};

use super::WindowId;
use super::font_manager::FontManager;
use super::icons::ChromeIcon;
use super::renderer::gl_facade::Gl;
use super::renderer::{ToastQuad, ToastRenderState, ToastTextRun};
use crate::gui::panes::PaneId;

mod buttons;
mod hit;
mod layout;
#[cfg(test)]
mod test_support;

use self::hit::ToastClickPart;

// ---------------------------------------------------------------------------
//  Animation constants
// ---------------------------------------------------------------------------
//
// The animation model and layout function below (`Toast::anim`, `ToastAnim`,
// `layout_toasts`, and their supporting types/constants) are wired into the
// fully-owned toast `PaintCallback` by [`ToastStack::show`] (issue #433).

/// Duration of the entry (fade + slide + scale in) animation.
const ANIM_IN: Duration = Duration::from_millis(300);

/// Duration of the exit (fade out) animation, applied to the last slice of a
/// toast's lifetime before it expires.
const ANIM_OUT: Duration = Duration::from_millis(400);

/// Horizontal distance, in logical points, a toast slides in from on entry.
/// For the (current default) right-anchored stack this is a slide-in from
/// further off to the right; [`Toast::anim`]'s `slide_x` is always toward
/// positive X (displaced right), and the layout function decides how that
/// maps onto each anchor.
const SLIDE_IN_PTS: f32 = 24.0;

/// Scale factor a toast animates in from on entry (1.0 = full size, settled).
const SCALE_IN: f32 = 0.92;

/// Grace window after the last hover during which a toast is held fully
/// visible (its removal timer paused). After this grace, a toast that hover
/// extended past its nominal life fades out over [`ANIM_OUT`]. Keeping this
/// small makes the toast start fading promptly once the pointer leaves.
const HOVER_HOLD: Duration = Duration::from_millis(200);

/// Severity of a toast.  Drives color and default duration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastKind {
    /// Non-fatal error that the user should see.
    Error,
    /// Warning that does not prevent continued operation. Used by the
    /// paste-blocked toast (routed via [`ToastStack::push_positioned`]).
    Warning,
    /// Informational message.
    Info,
}

impl ToastKind {
    /// Default lifetime before auto-dismissal, when not hovered.
    const fn default_duration(self) -> Duration {
        match self {
            Self::Error => Duration::from_secs(15),
            Self::Warning => Duration::from_secs(10),
            Self::Info => Duration::from_secs(6),
        }
    }

    /// Semantic accent color for this kind, taken directly from the active
    /// theme's palette semantic colors (error/warn/link) rather than a
    /// hard-coded hue. Used by the owned-renderer layout for the left
    /// accent-bar and icon tint ([`layout_toasts`](layout::layout_toasts) / `layout::toast_colors`).
    const fn semantic(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Self::Error => visuals.error_fg_color,
            Self::Warning => visuals.warn_fg_color,
            Self::Info => visuals.hyperlink_color,
        }
    }

    /// Icon glyph shown alongside this kind's toast in the owned-renderer
    /// layout ([`layout_toasts`](layout::layout_toasts)).
    ///
    /// `Error` and `Warning` both use [`ChromeIcon::Warning`] (an exclamation
    /// triangle) — the existing bundled icon set (see `icons.rs`) has no
    /// distinct "fatal" glyph, and the two kinds are already visually
    /// distinguished by their semantic accent color. `Info` reuses
    /// [`ChromeIcon::Bell`]: there is no dedicated "information" glyph in the
    /// bundled set either, and an informational toast is conceptually a
    /// notification, which the bell glyph reads as clearly at toast size.
    /// Adding a dedicated glyph per kind is out of scope for this subtask —
    /// `ChromeIcon` variants are not extended here.
    const fn icon(self) -> ChromeIcon {
        match self {
            Self::Error | Self::Warning => ChromeIcon::Warning,
            Self::Info => ChromeIcon::Bell,
        }
    }

    /// Solid background fill for the toast pill, derived from the active
    /// theme.
    ///
    /// Issue #433 visual redesign: the pill fill is the theme's neutral,
    /// fully-opaque window fill (the same color menus and dialogs use), *not*
    /// a saturated per-kind tint. The notification kind is expressed by the
    /// left accent bar and the semantic icon color, not by the whole
    /// background — so the pill reads as native chrome that sits calmly over
    /// the terminal rather than a jarring colored bubble. The fill is fully
    /// opaque (no alpha blending over the terminal) so the message text keeps
    /// maximum contrast and legibility.
    ///
    /// `window_fill` (rather than `panel_fill`) is deliberate: `window_fill`
    /// is always opaque even when the user has set a translucent
    /// `background_opacity` (which only affects `panel_fill`), so a toast is
    /// never see-through.
    fn background(visuals: &egui::Visuals) -> egui::Color32 {
        visuals.window_fill.to_opaque()
    }

    /// Whether `bg` is light enough that dark text reads better on it.
    fn prefers_dark_text(bg: egui::Color32) -> bool {
        // Rec. 601 luma; > 0.6 → light fill → use dark text.
        let r = f32::from(bg.r()) / 255.0;
        let g = f32::from(bg.g()) / 255.0;
        let b = f32::from(bg.b()) / 255.0;
        0.299_f32.mul_add(r, 0.587_f32.mul_add(g, 0.114 * b)) > 0.6
    }

    /// Short prefix shown before the message (e.g. "Error").
    const fn label(self) -> &'static str {
        match self {
            Self::Error => "Error",
            Self::Warning => "Warning",
            Self::Info => "Info",
        }
    }
}

/// Opaque caller-chosen identity of a keyed toast. The toast module attaches
/// no meaning to the value; it is only compared for equality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ToastKey(pub u64);

/// How long a toast stays on screen (when not hovered).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastLifetime {
    /// The toast kind's default ([`ToastKind::default_duration`]). Every
    /// unkeyed toast uses this.
    KindDefault,
    /// An explicit duration.
    Fixed(Duration),
    /// Never expires; only a click, eviction or [`ToastStack::close_keyed`]
    /// removes it.
    UntilDismissed,
}

/// What happened to a keyed toast. See [`ToastEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastEventKind {
    /// The user clicked the toast body (always followed by `Closed`).
    Activated,
    /// The user clicked button _n_ (1-based, in label order; always followed
    /// by `Closed`).
    Button(u8),
    /// The toast left the stack, for any reason.
    Closed,
}

/// An event produced by a keyed toast, drained with
/// [`ToastStack::take_events`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ToastEvent {
    /// The key the toast was pushed with.
    pub key: ToastKey,
    /// What happened.
    pub kind: ToastEventKind,
}

/// Outcome of [`ToastStack::replace_keyed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ToastReplace {
    /// A toast with the old key existed and was updated in place.
    Replaced,
    /// No toast had the old key; a new one was pushed instead.
    Pushed,
}

/// The most buttons a toast keeps. Further labels are dropped (the PTY side
/// already caps at the same number; this is a defensive second cap).
const MAX_TOAST_BUTTONS: usize = 8;

/// What a keyed toast shows: its severity, text and button labels.
///
/// Bundled so [`ToastStack::push_keyed`] and [`ToastStack::replace_keyed`]
/// stay under the `too_many_arguments` limit. Unkeyed toasts have no buttons.
#[derive(Debug, Clone)]
pub(super) struct ToastContent {
    /// The severity (colour and icon).
    pub kind: ToastKind,
    /// One-line headline.
    pub title: String,
    /// Optional detail line.
    pub detail: Option<String>,
    /// Button labels, in order. Only the first [`MAX_TOAST_BUTTONS`] are kept.
    pub buttons: Vec<String>,
    /// Icon drawn instead of the kind's own icon. `None` keeps the kind
    /// icon; the accent colour follows the kind either way.
    pub icon: Option<ChromeIcon>,
}

/// A single toast entry in the stack.
#[derive(Debug, Clone)]
pub(super) struct Toast {
    kind: ToastKind,
    /// One-line headline (bold).
    title: String,
    /// Optional multi-line detail (wrapped).
    detail: Option<String>,
    /// Button labels (keyed toasts only; at most [`MAX_TOAST_BUTTONS`]).
    buttons: Vec<String>,
    /// Icon override; `None` shows the kind's own icon (see [`Self::icon`]).
    icon: Option<ChromeIcon>,
    /// Monotonic id used as the egui widget id seed, so each toast has
    /// a stable id across frames even after list reordering.
    id: u64,
    /// When the toast was created.  Drives the entry animation only; a
    /// keyed replace keeps it so the toast does not re-enter.
    created: Instant,
    /// When the toast's lifetime started: `created` for a new toast, reset by
    /// [`ToastStack::replace_keyed`]. Used with [`Self::lifetime`] to compute
    /// expiry and the exit fade (unless hovered).
    life_start: Instant,
    /// How long the toast lives (see [`ToastLifetime`]).
    lifetime: ToastLifetime,
    /// Caller identity for a keyed toast; `None` for ordinary toasts, which
    /// never produce [`ToastEvent`]s.
    key: Option<ToastKey>,
    /// When the toast was last hovered.  `None` if never hovered.  Used
    /// to extend the lifetime of a toast the user is reading.
    last_hovered: Option<Instant>,
    /// Anchor placement (and, for a pane-centered toast, its window+pane
    /// origin). Defaults to [`ToastPlacement::TOP_RIGHT`] for toasts pushed
    /// without an explicit placement.
    placement: ToastPlacement,
}

impl Toast {
    fn new(
        kind: ToastKind,
        title: String,
        detail: Option<String>,
        id: u64,
        placement: ToastPlacement,
    ) -> Self {
        let now = Instant::now();
        Self {
            kind,
            title,
            detail,
            buttons: Vec::new(),
            icon: None,
            id,
            created: now,
            life_start: now,
            lifetime: ToastLifetime::KindDefault,
            key: None,
            last_hovered: None,
            placement,
        }
    }

    /// Make this a keyed toast with the given lifetime.
    const fn keyed(mut self, key: ToastKey, lifetime: ToastLifetime) -> Self {
        self.key = Some(key);
        self.lifetime = lifetime;
        self
    }

    /// Override the icon drawn in the toast's icon slot.
    const fn with_icon(mut self, icon: Option<ChromeIcon>) -> Self {
        self.icon = icon;
        self
    }

    /// The glyph drawn in the icon slot: the override when there is one,
    /// otherwise the kind's icon. The single source for both measuring and
    /// drawing.
    const fn icon(&self) -> ChromeIcon {
        match self.icon {
            Some(icon) => icon,
            None => self.kind.icon(),
        }
    }

    /// Give this toast button labels, keeping at most [`MAX_TOAST_BUTTONS`].
    fn with_buttons(mut self, mut buttons: Vec<String>) -> Self {
        buttons.truncate(MAX_TOAST_BUTTONS);
        self.buttons = buttons;
        self
    }

    /// Total nominal lifetime, or `None` if the toast never expires on its
    /// own ([`ToastLifetime::UntilDismissed`]).
    const fn total_lifetime(&self) -> Option<Duration> {
        match self.lifetime {
            ToastLifetime::KindDefault => Some(self.kind.default_duration()),
            ToastLifetime::Fixed(duration) => Some(duration),
            ToastLifetime::UntilDismissed => None,
        }
    }

    /// This toast's anchor placement (see [`ToastPosition`]).
    pub(super) const fn position(&self) -> ToastPosition {
        self.placement.position
    }

    /// This toast's origin `(WindowId, PaneId)`, if any (see the field docs).
    pub(super) const fn origin(&self) -> Option<(WindowId, PaneId)> {
        self.placement.origin
    }

    /// Returns `true` if the toast should be removed this frame.
    ///
    /// A toast is removed once it is both past its nominal lifetime AND past
    /// the trailing fade-out window. The fade-out window is anchored so it is
    /// always fully visible: for a non-hovered toast it is the last
    /// [`ANIM_OUT`] of its nominal life; for a toast whose hover extended it
    /// past nominal life, it is the [`ANIM_OUT`] following the moment hover
    /// ended (plus the [`HOVER_HOLD`] grace). Keeping this in lock-step with
    /// [`Self::anim`] guarantees a toast is never removed while `anim` still
    /// shows it fading (review finding #2).
    fn is_expired(&self, now: Instant) -> bool {
        // An `UntilDismissed` toast never expires on its own.
        let Some(total) = self.total_lifetime() else {
            return false;
        };
        // While actively hovered (within the hold grace), never expire.
        if self.hovered_recently(now) {
            return false;
        }
        let age = now.saturating_duration_since(self.life_start);
        // Not yet past nominal life -> alive.
        if age <= total {
            return false;
        }
        // Past nominal life. If hover extended it, allow a full ANIM_OUT
        // fade after the hold grace ends before removing it.
        self.last_hovered.is_none_or(|hover| {
            now.saturating_duration_since(hover) >= HOVER_HOLD.saturating_add(ANIM_OUT)
        })
    }

    /// Whether `now` falls within the active-hover hold grace, during which
    /// the toast is held fully visible and its timer does not advance toward
    /// removal. Mirrors the condition [`Self::is_expired`] / [`Self::anim`]
    /// use to pause auto-dismissal.
    fn hovered_recently(&self, now: Instant) -> bool {
        self.last_hovered
            .is_some_and(|hover| now.saturating_duration_since(hover) < HOVER_HOLD)
    }

    /// Compute this toast's animation phase at `now`.
    ///
    /// Phases, in order:
    /// 1. **Entry** (`age < ANIM_IN`): eases in from `(opacity=0, slide_x=
    ///    SLIDE_IN_PTS, scale=SCALE_IN)` to the settled state via an
    ///    ease-out cubic.
    /// 2. **Active hover hold** (within [`HOVER_HOLD`] of the last hover):
    ///    fully settled — the toast is being read, its exit is paused.
    /// 3. **Post-hover exit** (hovered past its nominal life, hover now
    ///    ended): fades opacity `1 -> 0` linearly over [`ANIM_OUT`] measured
    ///    from the end of the [`HOVER_HOLD`] grace. This is the case the old
    ///    model got wrong — it popped instantly (review finding #2).
    /// 4. **End-of-life exit** (within the last [`ANIM_OUT`] of nominal life,
    ///    never hover-extended): fades opacity `1 -> 0` over the remaining
    ///    time.
    /// 5. **Hold**: fully settled, `(opacity=1, slide_x=0, scale=1)`.
    ///
    /// Phases 2-4 keep `anim` in lock-step with [`Self::is_expired`] so a
    /// toast is never removed mid-fade nor left visible after fading to zero.
    /// All outputs are clamped to their documented ranges.
    fn anim(&self, now: Instant) -> ToastAnim {
        const SETTLED: ToastAnim = ToastAnim {
            opacity: 1.0,
            slide_x: 0.0,
            scale: 1.0,
        };
        // A pure fade-out (no slide/scale) at opacity `q`.
        let fade = |q: f32| ToastAnim {
            opacity: q.clamp(0.0, 1.0),
            slide_x: 0.0,
            scale: 1.0,
        };

        // Entry progress runs from `created`; the exit timeline runs from
        // `life_start` (they differ only after a keyed replace). `total` uses
        // the same lifetime `is_expired` does.
        let entry_age = now.saturating_duration_since(self.created);
        let age = now.saturating_duration_since(self.life_start);
        let total = self.total_lifetime();

        // Degenerate timelines (not expected in practice — every
        // `ToastKind::default_duration()` is nonzero — but guards against
        // div-by-zero if the animation durations are ever misconfigured).
        if ANIM_IN.is_zero() || ANIM_OUT.is_zero() || total.as_ref().is_some_and(Duration::is_zero)
        {
            return SETTLED;
        }

        // Phase 1: entry.
        if entry_age < ANIM_IN {
            let p = (entry_age.as_secs_f32() / ANIM_IN.as_secs_f32()).clamp(0.0, 1.0);
            let eased = 1.0 - (1.0 - p).powi(3);
            return ToastAnim {
                opacity: eased.clamp(0.0, 1.0),
                slide_x: (SLIDE_IN_PTS * (1.0 - eased)).clamp(0.0, SLIDE_IN_PTS),
                scale: SCALE_IN.mul_add(1.0 - eased, eased).clamp(0.0, 1.0),
            };
        }

        // Phase 2: actively hovered — held fully visible.
        if self.hovered_recently(now) {
            return SETTLED;
        }

        // `UntilDismissed` has no exit timeline: it stays settled.
        let Some(total) = total else {
            return SETTLED;
        };

        // Phase 3: post-hover exit. If a hover extended the toast past its
        // nominal life, fade over ANIM_OUT starting when the HOVER_HOLD grace
        // ends (mirrors `is_expired`'s removal at HOVER_HOLD + ANIM_OUT after
        // the last hover).
        if age > total
            && let Some(hover) = self.last_hovered
        {
            let since_hover = now.saturating_duration_since(hover);
            let into_fade = since_hover.saturating_sub(HOVER_HOLD);
            let remaining = ANIM_OUT.saturating_sub(into_fade);
            return fade(remaining.as_secs_f32() / ANIM_OUT.as_secs_f32());
        }

        // Phase 4: end-of-life exit (never hover-extended).
        let remaining = total.saturating_sub(age);
        if remaining < ANIM_OUT {
            return fade(remaining.as_secs_f32() / ANIM_OUT.as_secs_f32());
        }

        // Phase 5: hold.
        SETTLED
    }
}

/// One toast's animation state at a point in time, as computed by
/// [`Toast::anim`]. All values are already clamped to their documented
/// ranges.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ToastAnim {
    /// Overall opacity multiplier, `0..=1`.
    pub opacity: f32,
    /// Horizontal displacement in logical points. `0` when settled; positive
    /// values displace the toast toward positive X (off-screen, for a
    /// right-anchored stack).
    pub slide_x: f32,
    /// Uniform scale factor, `0..=1` (`1` = full size/settled).
    pub scale: f32,
}

/// Where the toast stack is anchored within the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ToastPosition {
    /// Stacked in the top-right corner of the window — the default placement
    /// for toasts pushed without an explicit [`ToastPlacement`] (errors,
    /// info, and OSC 9/777/99/133D notifications). See `layout::resolve_anchor`.
    #[default]
    TopRightStack,
    /// The whole stack centered within the window's content rect. Used for
    /// freminal-derived toasts with no pane origin, or whose pane origin is
    /// in a window other than the one currently being rendered — see
    /// `layout::resolve_anchor`.
    WindowCentered,
    /// Centered within the originating pane's rect, resolved in the toast's
    /// origin window (see [`Toast::origin`]). Falls back to
    /// [`ToastPosition::WindowCentered`] when the pane is not found in the
    /// window currently being rendered (foreign window, no origin, or the
    /// pane has since closed) — see `layout::resolve_anchor`.
    PaneCentered,
}

/// Where a toast is anchored plus, for a pane-centered toast, which
/// window+pane it originates from.
///
/// Bundled so the toast push/route APIs that carry a placement stay under
/// the `too_many_arguments` limit (mirroring [`ToastMeasureSizes`] /
/// [`ToastLayoutMetrics`](layout::ToastLayoutMetrics)). `origin` is consulted only when `position` is
/// [`ToastPosition::PaneCentered`]; it is resolved to a pane rect at render
/// time in the toast's origin window and falls back to window-centered in
/// any other window (or when `None`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct ToastPlacement {
    /// The anchor placement (top-right stack, window-centered, pane-centered).
    pub position: ToastPosition,
    /// The originating `(WindowId, PaneId)`, if any. See the struct docs.
    pub origin: Option<(WindowId, PaneId)>,
}

impl ToastPlacement {
    /// The default top-right stacked placement with no origin — used by the
    /// severity convenience pushers ([`ToastStack::error`] / [`info`]) and by
    /// any toast that does not opt into centered positioning.
    pub(super) const TOP_RIGHT: Self = Self {
        position: ToastPosition::TopRightStack,
        origin: None,
    };

    /// A window-centered placement with no pane origin — used by
    /// freminal-derived, app/window-level toasts (recording, layout, config
    /// reload, remote clipboard, paste-blocked) that are not tied to a
    /// specific pane.
    pub(super) const WINDOW_CENTERED: Self = Self {
        position: ToastPosition::WindowCentered,
        origin: None,
    };

    /// A pane-centered placement originating from `(window_id, pane_id)` —
    /// used by pane-specific toasts (e.g. "Copied to clipboard") so they
    /// center within their originating pane in that window.
    pub(super) const fn pane_centered(window_id: WindowId, pane_id: PaneId) -> Self {
        Self {
            position: ToastPosition::PaneCentered,
            origin: Some((window_id, pane_id)),
        }
    }
}

/// Ordered stack of active toasts, rendered top-to-bottom from the most
/// recent.  Capped at [`MAX_TOASTS`] entries; older entries are evicted
/// when the cap is exceeded.
#[derive(Debug, Default)]
pub(super) struct ToastStack {
    entries: Vec<Toast>,
    next_id: u64,
    /// Cached "is any toast currently mid-animation" signal (subtask
    /// 121.14), recomputed at the end of every [`Self::show`] call and
    /// consulted by [`Self::is_animating`]. `App::pointer_motion_needs_repaint`
    /// runs OUTSIDE any frame (from `event_loop`'s `CursorMoved` handling),
    /// where [`Self::measure_inputs`] cannot be called — this field is what
    /// lets that method see the answer without re-running the per-frame
    /// measurement. `false` on a fresh/default stack (nothing to animate).
    cached_is_animating: bool,
    /// Events produced by keyed toasts since the last
    /// [`Self::take_events`]. Unkeyed toasts never add to this.
    events: Vec<ToastEvent>,
}

/// Maximum simultaneous toasts.  Older ones are evicted.
const MAX_TOASTS: usize = 5;

/// The font sizes (physical pixels) and `pixels_per_point` scale
/// [`ToastStack::measure_inputs`] needs, bundled into one struct so that
/// function stays under the `too_many_arguments` limit.
#[derive(Debug, Clone, Copy)]
struct ToastMeasureSizes {
    /// Physical pixel size the label is rasterized at.
    label_size_px: f32,
    /// Physical pixel size the detail line is rasterized at.
    detail_size_px: f32,
    /// Physical pixel size the icon glyph is rasterized at.
    icon_size_px: f32,
    /// `pixels_per_point`, needed to convert the logical-point pill-geometry
    /// constants into the same physical-pixel space the measured text
    /// widths live in (see `layout::text_width_budget`).
    ppp: f32,
}

/// The per-frame GL/text resources [`ToastStack::show`] needs, bundled into
/// one struct so `show` stays under the `too_many_arguments` limit (mirroring
/// [`ToastMeasureSizes`]). `render_state` is the window's toast GL state
/// (per-window, since GL resources belong to one GL context); `font_manager`
/// is the window's shared [`FontManager`] (obtained via
/// `FreminalTerminalWidget::font_manager_mut`), used only for
/// measuring/shaping — never touched from inside the `PaintCallback`.
pub(super) struct ToastFrameResources<'a> {
    /// This window's toast GL render state.
    pub render_state: &'a std::sync::Arc<std::sync::Mutex<ToastRenderState>>,
    /// This window's shared font manager, for text measure/shape only.
    pub font_manager: &'a mut FontManager,
}

/// [`ToastStack::show`]'s return value (subtask 121.14): bundles the
/// repaint delay it needs (subtask 121.12) with the laid-out pill hit-rects,
/// so the caller can cache both without a second pass over the stack.
pub(super) struct ToastShowOutcome {
    /// The delay `App::update` should schedule for this window this frame
    /// (subtask 121.12) — see [`ToastStack::show`]'s doc.
    pub(super) repaint_delay: Option<Duration>,
    /// Every toast's `hit_rect_logical`, converted to `egui::Rect` (logical
    /// points, window space) — see [`ToastStack::show`]'s doc for why the
    /// caller needs these.
    pub(super) rects: Vec<egui::Rect>,
}

impl ToastShowOutcome {
    /// The outcome for a stack with nothing to show: no delay requested, no
    /// rects to cache.
    const fn empty() -> Self {
        Self {
            repaint_delay: None,
            rects: Vec::new(),
        }
    }
}

impl ToastStack {
    /// Push a new error toast onto the stack (top-right stacked).
    pub(super) fn error(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(
            ToastKind::Error,
            title.into(),
            detail,
            ToastPlacement::TOP_RIGHT,
        );
    }

    /// Push a new informational toast onto the stack (top-right stacked).
    pub(super) fn info(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(
            ToastKind::Info,
            title.into(),
            detail,
            ToastPlacement::TOP_RIGHT,
        );
    }

    /// Push a toast with an explicit anchor placement (see [`ToastPlacement`]).
    /// Used by freminal-derived toast routing to center toasts (pane- or
    /// window-centered) rather than stacking them top-right.
    pub(super) fn push_positioned(
        &mut self,
        kind: ToastKind,
        title: impl Into<String>,
        detail: Option<String>,
        placement: ToastPlacement,
    ) {
        self.push(kind, title.into(), detail, placement);
    }

    /// Number of toasts currently on the stack. Test-only helper used by the
    /// notification router tests to assert toast-leg routing decisions.
    #[cfg(test)]
    pub(super) const fn len(&self) -> usize {
        self.entries.len()
    }

    /// The kind of the last (most-recently pushed) toast, if any. Test-only
    /// accessor (also used by the notification-router tests to assert
    /// freminal-toast severity mapping).
    #[cfg(test)]
    pub(super) fn last_kind(&self) -> Option<ToastKind> {
        self.entries.last().map(|t| t.kind)
    }

    /// Whether the stack currently has no toasts. Used by the frame-damage
    /// aggregation (#435): a visible toast animates its own region each
    /// frame, so its presence forces a full-frame present.
    pub(super) const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Whether any toast on the stack is currently mid-animation (entry/exit
    /// fade+slide+scale), as of the most recent [`Self::show`] call or
    /// [`Self::push`] (subtask 121.14).
    ///
    /// Mere presence (`!is_empty()`) is NOT sufficient grounds to treat a
    /// toast as needing continuous repaints — a toast spends most of its
    /// 1-3s (up to 15s) life fully settled in its steady hold, animating
    /// only during its 300ms entry fade and 400ms exit fade. Callers that
    /// need "is a repaint needed to keep this toast's animation smooth"
    /// (as opposed to "is a toast visible at all", which has its own,
    /// deliberately presence-based, use at `app_impl.rs`'s
    /// `toast_active` — see that field's doc) must combine this with
    /// `!is_empty()` themselves.
    pub(super) const fn is_animating(&self) -> bool {
        self.cached_is_animating
    }

    /// Push a keyed toast (see [`ToastKey`]) with an explicit `lifetime`.
    ///
    /// Its [`ToastEvent`]s are collected by [`Self::take_events`]. The key is
    /// not checked for uniqueness; callers that update a toast should use
    /// [`Self::replace_keyed`].
    pub(super) fn push_keyed(
        &mut self,
        key: ToastKey,
        content: ToastContent,
        lifetime: ToastLifetime,
        placement: ToastPlacement,
    ) {
        let ToastContent {
            kind,
            title,
            detail,
            buttons,
            icon,
        } = content;
        let toast = Toast::new(kind, title, detail, self.alloc_id(), placement)
            .keyed(key, lifetime)
            .with_icon(icon)
            .with_buttons(buttons);
        self.push_entry(toast);
    }

    /// Update the toast keyed `old` in place, re-keying it to `new_key`.
    ///
    /// The toast keeps its stack slot, placement and `created` instant (so no
    /// entry animation replays), takes the new content and `lifetime`, and
    /// **its lifetime restarts**: keeping the age would make a toast updated
    /// every second expire mid-update. No event is produced for `old`.
    ///
    /// If no toast is keyed `old`, behaves as [`Self::push_keyed`] with
    /// [`ToastPlacement::TOP_RIGHT`] and returns [`ToastReplace::Pushed`].
    /// (No placement parameter: with one this method would exceed the
    /// `too_many_arguments` limit, and a replaced toast always keeps its own.)
    pub(super) fn replace_keyed(
        &mut self,
        old: ToastKey,
        new_key: ToastKey,
        content: ToastContent,
        lifetime: ToastLifetime,
    ) -> ToastReplace {
        self.replace_keyed_at(Instant::now(), old, new_key, content, lifetime)
    }

    /// [`Self::replace_keyed`] with an injected `now` (the new `life_start`),
    /// so the lifetime reset is testable without sleeping.
    fn replace_keyed_at(
        &mut self,
        now: Instant,
        old: ToastKey,
        new_key: ToastKey,
        content: ToastContent,
        lifetime: ToastLifetime,
    ) -> ToastReplace {
        let Some(toast) = self.entries.iter_mut().find(|t| t.key == Some(old)) else {
            self.push_keyed(new_key, content, lifetime, ToastPlacement::TOP_RIGHT);
            return ToastReplace::Pushed;
        };
        let ToastContent {
            kind,
            title,
            detail,
            mut buttons,
            icon,
        } = content;
        buttons.truncate(MAX_TOAST_BUTTONS);
        toast.key = Some(new_key);
        toast.kind = kind;
        toast.title = title;
        toast.detail = detail;
        toast.buttons = buttons;
        toast.icon = icon;
        toast.lifetime = lifetime;
        toast.life_start = now;
        ToastReplace::Replaced
    }

    /// Remove every toast keyed `key`, recording `Closed` for each. A no-op
    /// (no event) if none exists.
    pub(super) fn close_keyed(&mut self, key: ToastKey) {
        let mut closed = Vec::new();
        self.entries.retain(|t| {
            if t.key == Some(key) {
                closed.push(key);
                false
            } else {
                true
            }
        });
        self.record_closed(closed);
    }

    /// Drain the events recorded by keyed toasts since the last call.
    pub(super) fn take_events(&mut self) -> Vec<ToastEvent> {
        std::mem::take(&mut self.events)
    }

    /// Record a `Closed` event for each of `keys`.
    fn record_closed(&mut self, keys: impl IntoIterator<Item = ToastKey>) {
        self.events.extend(keys.into_iter().map(|key| ToastEvent {
            key,
            kind: ToastEventKind::Closed,
        }));
    }

    /// Remove every toast that has expired at `now`, recording `Closed` for
    /// each keyed one.
    fn sweep_expired(&mut self, now: Instant) {
        let mut closed = Vec::new();
        self.entries.retain(|t| {
            if t.is_expired(now) {
                closed.extend(t.key);
                false
            } else {
                true
            }
        });
        self.record_closed(closed);
    }

    /// Handle a primary click on toast `id` at `part`: the toast is removed,
    /// and a keyed toast records `Activated` (body) or `Button(n)` (button
    /// _n_) and then `Closed`; the dismiss square records `Closed` only.
    /// Unkeyed toasts are removed with no event, whichever part was hit. A
    /// no-op if `id` is not on the stack.
    fn apply_click(&mut self, id: u64, part: ToastClickPart) {
        let Some(index) = self.entries.iter().position(|t| t.id == id) else {
            return;
        };
        let toast = self.entries.remove(index);
        let Some(key) = toast.key else {
            return;
        };
        let activation = match part {
            ToastClickPart::Body => Some(ToastEventKind::Activated),
            ToastClickPart::Button(n) => Some(ToastEventKind::Button(n)),
            ToastClickPart::Dismiss => None,
        };
        self.events
            .extend(activation.map(|kind| ToastEvent { key, kind }));
        self.record_closed([key]);
    }

    /// Allocate the next monotonic toast id.
    const fn alloc_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    fn push(
        &mut self,
        kind: ToastKind,
        title: String,
        detail: Option<String>,
        placement: ToastPlacement,
    ) {
        let toast = Toast::new(kind, title, detail, self.alloc_id(), placement);
        self.push_entry(toast);
    }

    /// Append `toast`, evicting the oldest entries beyond [`MAX_TOASTS`]
    /// (recording `Closed` for each evicted keyed toast).
    fn push_entry(&mut self, toast: Toast) {
        self.entries.push(toast);
        // Evict oldest entries beyond the cap.
        while self.entries.len() > MAX_TOASTS {
            let evicted = self.entries.remove(0);
            self.record_closed(evicted.key);
        }
        // Subtask 121.14: eagerly mark the stack as animating. A freshly
        // pushed toast is always at the start of its entry animation (age
        // 0 < `ANIM_IN`), so this is exact, not merely conservative. It
        // closes the priming gap between this `push()` and the next
        // `show()` call (the only other place `cached_is_animating` is
        // set): without it, a pointer-motion check landing in that window
        // would read whatever the field held before the push -- `false` if
        // the stack had been empty -- and wrongly suppress a repaint the
        // entry animation's first frames need.
        self.cached_is_animating = true;
    }

    /// Font size (physical pixels, pre-`pixels_per_point` scaling) the toast
    /// label is rasterized at. See [`Self::show`].
    const LABEL_SIZE_PTS: f32 = 14.0;
    /// Font size (physical pixels, pre-`pixels_per_point` scaling) the toast
    /// detail line is rasterized at. See [`Self::show`].
    const DETAIL_SIZE_PTS: f32 = 12.0;

    /// Render the stack as a fully-owned GL overlay (issue #433): measures
    /// and shapes every toast's text on the main thread, lays it out via
    /// [`layout_toasts`](layout::layout_toasts), and hands the result to a single `PaintCallback`
    /// that draws every pill, then every text run, on top of everything
    /// else in the window.
    ///
    /// `content_rect` is this window's central-panel content rect (the
    /// window minus chrome). `this_window_id` is the window currently being
    /// rendered and `resolve_pane_rect` resolves a [`PaneId`] to its rect in
    /// that window (or `None` if the pane is not there) — all feed
    /// `layout::resolve_anchor` so each toast's [`ToastPosition`] (top-right stack
    /// / window-centered / pane-centered) is resolved per-window rather than
    /// hardcoded (see [`Self::layout_positioned`]).
    ///
    /// `resources` bundles this window's toast GL state and font manager (see
    /// [`ToastFrameResources`]), used only for measuring/shaping — never
    /// touched from inside the `PaintCallback`.
    ///
    /// Clears auto-expired toasts and any the user dismissed by clicking
    /// anywhere within a toast's pill (its `hit_rect_logical`, computed by
    /// `layout_toasts`). Keyed toasts record their [`ToastEvent`]s as they
    /// go (see [`Self::take_events`]).
    ///
    /// Returns a [`ToastShowOutcome`] bundling the repaint delay this call
    /// needs to keep expiry/animation going without further input (subtask
    /// 121.12: `Some(16ms)` while any toast is animating, `Some(250ms)` if
    /// the stack is non-empty but settled, `None` when empty) and the laid-
    /// out pill hit-rects (subtask 121.14, logical points, window space —
    /// same rects [`Self::hit_test`] hit-tests against). THE CALLER MUST
    /// SCHEDULE THE RETURNED DELAY AND CACHE THE RETURNED RECTS — this
    /// method deliberately does NOT call `ctx.request_repaint_after()`
    /// itself, because that would be invisible to `effective_repaint_delay`'s
    /// suppressed-pointer substitution (in `freminal-windowing`) and would be
    /// silently downgraded to the fallback interval while the mouse is
    /// moving over terminal content; and the rects are needed by
    /// `App::pointer_motion_needs_repaint`, which runs outside any frame and
    /// so cannot call this method itself.
    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        content_rect: egui::Rect,
        this_window_id: WindowId,
        resolve_pane_rect: impl Fn(PaneId) -> Option<egui::Rect>,
        resources: ToastFrameResources<'_>,
        pixels_per_point: f32,
    ) -> ToastShowOutcome {
        if self.entries.is_empty() {
            self.cached_is_animating = false;
            return ToastShowOutcome::empty();
        }

        let ToastFrameResources {
            render_state,
            font_manager,
        } = resources;

        let now = Instant::now();
        let ppp = pixels_per_point;
        let visuals = ctx.global_style().visuals.clone();

        let label_size_px = Self::LABEL_SIZE_PTS * ppp;
        let detail_size_px = Self::DETAIL_SIZE_PTS * ppp;
        let icon_size_px = label_size_px;
        let sizes = ToastMeasureSizes {
            label_size_px,
            detail_size_px,
            icon_size_px,
            ppp,
        };

        // Measure + shape every toast's text on the main thread; the lock is
        // held only for the duration of this call — no GL is touched.
        let (inputs, any_animating) = {
            let rs = render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.measure_inputs(&rs, font_manager, sizes, now)
        };

        // `layout_toasts` emits window-origin-absolute physical-pixel
        // coordinates (it scales the resolved geometry's rect directly,
        // without subtracting its `min`) — see its own field docs. The
        // `PaintCallback` registered by `paint_toasts` below uses
        // `rect: ctx.content_rect()` (the full window, origin `(0,0)`), so
        // its GL viewport starts at the window's own origin and these
        // coordinates need no further translation to land in the right
        // place. Each toast's actual anchor is resolved per-window by
        // `layout_positioned` (via `resolve_anchor`) rather than hardcoded.
        let outputs = self.layout_positioned(
            &inputs,
            this_window_id,
            content_rect,
            &resolve_pane_rect,
            &visuals,
            ppp,
        );

        // Owned hover/dismiss hit-testing via a real interactive egui region
        // per toast (in LOGICAL points, window space). Registering an actual
        // `Sense::click()` widget — rather than passively reading pointer
        // state — is what makes egui's input arbitration *consume* a dismiss
        // click, so the terminal pane beneath the toast never also receives it
        // (which would otherwise start a selection or leak a mouse-report to
        // the running program). Clicks outside every toast rect are never
        // allocated, so they fall through to the terminal untouched. Toasts
        // are not a typing modal, so this deliberately does NOT touch the
        // `ui_overlay_open` / `suppress_input` global gate — only the exact
        // click that lands on a toast is consumed. (Review finding #3.)
        self.hit_test(ctx, &outputs, now);

        let text_instances = {
            let mut rs = render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let runs: Vec<ToastTextRun> = outputs.iter().flat_map(|o| o.runs.clone()).collect();
            rs.text.build_instances(&runs, font_manager)
        };
        // Draw order: each pill, then its buttons, then the next pill.
        let pills: Vec<ToastQuad> = outputs
            .iter()
            .flat_map(|o| std::iter::once(o.pill).chain(o.button_quads.iter().copied()))
            .collect();
        // Subtask 121.14: the same hit rects `hit_test` above just tested the
        // pointer against, in the same logical-point/window-space coordinates
        // `PublishedFrameState`'s `chrome_head_rects`/`chrome_border_rects`
        // use — cached by the caller for `App::pointer_motion_needs_repaint`
        // (see this method's doc). Computed from `outputs`, which describes
        // what was ACTUALLY rendered this frame (before the dismiss/expire
        // retains below), so it matches the pills just painted rather than
        // what remains after.
        let rects: Vec<egui::Rect> = outputs
            .iter()
            .map(|o| {
                let [min_x, min_y, max_x, max_y] = o.hit_rect_logical;
                egui::Rect::from_min_max(egui::pos2(min_x, min_y), egui::pos2(max_x, max_y))
            })
            .collect();
        Self::paint_toasts(ctx, render_state, pills, text_instances);

        // Expire toasts (dismissed ones were already removed by `hit_test`).
        self.sweep_expired(now);

        // Report the delay needed so expiry/animation happens even without
        // input: a fast (16ms, ~60fps) cadence while any toast is animating
        // (entry/exit fade+slide+scale), otherwise a slow (250ms) cadence
        // just to catch expiry. The caller schedules this (subtask 121.12) —
        // see this method's doc comment for why.
        //
        // Also caches `is_animating` (subtask 121.14) from the SAME
        // `entries.is_empty()` / `any_animating` decision, so the two never
        // disagree.
        let repaint_delay = if self.entries.is_empty() {
            self.cached_is_animating = false;
            None
        } else if any_animating {
            self.cached_is_animating = true;
            Some(Duration::from_millis(16))
        } else {
            self.cached_is_animating = false;
            Some(Duration::from_millis(250))
        };
        ToastShowOutcome {
            repaint_delay,
            rects,
        }
    }

    /// Register the single `PaintCallback` that draws every toast pill,
    /// then every toast text run, on top of the rest of the window. Split
    /// out of [`Self::show`] to keep it under the line-count limit.
    fn paint_toasts(
        ctx: &egui::Context,
        render_state: &std::sync::Arc<std::sync::Mutex<ToastRenderState>>,
        pills: Vec<ToastQuad>,
        text_instances: Vec<f32>,
    ) {
        let render_state_cb = std::sync::Arc::clone(render_state);
        let layer_painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("toast_overlay_gl"),
        ));
        layer_painter.add(egui::PaintCallback {
            rect: ctx.content_rect(),
            callback: std::sync::Arc::new(egui_glow::CallbackFn::new(move |info, painter| {
                let gl = &Gl::real(painter.gl());
                let vp = info.viewport_in_pixels();
                let mut rs = render_state_cb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // Failed inits are latched: logged once, not retried every
                // frame. The error is only logged -- a toast cannot report
                // that the toast renderer is broken.
                if rs.pill.should_attempt_init()
                    && let Err(e) = rs.pill.init(gl)
                {
                    tracing::error!("toast pill GL init failed: {e}");
                }
                if rs.text.should_attempt_init()
                    && let Err(e) = rs.text.init(gl)
                {
                    tracing::error!("toast text GL init failed: {e}");
                }
                if !rs.pill.initialized() || !rs.text.initialized() {
                    return;
                }
                rs.pill.draw(gl, &pills, vp.width_px, vp.height_px);
                rs.text
                    .upload_and_draw(gl, &text_instances, vp.width_px, vp.height_px);
            })),
        });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::gui::toast::test_support::SETTLED_ANIM;

    #[test]
    fn stack_starts_empty() {
        let s = ToastStack::default();
        assert!(s.entries.is_empty());
    }

    // ── Subtask 121.14: `ToastStack::is_animating` / `cached_is_animating` ──
    //
    // `ToastStack::show` needs a live `WindowId` (no public constructor
    // outside the real winit event loop — see `layout_positioned`'s doc), so
    // it cannot be driven headlessly here. These tests instead cover the
    // reachable half: the cached flag's default and `push`'s eager-`true`
    // write. The `show()`-side half (recomputing the flag from
    // `any_animating`) is covered instead at the `measure_inputs` level by
    // `measure_inputs_leaves_a_short_toast_unchanged` /
    // `measure_inputs_truncates_long_detail_and_pill_stays_within_max_width`
    // above, which assert `any_animating` directly.

    #[test]
    fn cached_is_animating_starts_false_on_a_default_stack() {
        let s = ToastStack::default();
        assert!(!s.is_animating());
    }

    #[test]
    fn push_sets_cached_is_animating_true() {
        let mut s = ToastStack::default();
        assert!(!s.is_animating());
        s.error("something failed", None);
        // Subtask 121.14: exact, not conservative — a freshly pushed toast
        // is always at the start of its entry animation.
        assert!(s.is_animating());
    }

    #[test]
    fn push_sets_cached_is_animating_true_even_when_stack_was_already_non_empty() {
        let mut s = ToastStack::default();
        s.info("first", None);
        // Simulate a `show()` having already settled the flag back to
        // `false` (the first toast finished its entry fade).
        s.cached_is_animating = false;
        s.error("second", None);
        assert!(
            s.is_animating(),
            "a second push must re-arm the flag even if the stack was \
             already non-empty and settled"
        );
    }

    #[test]
    fn prefers_dark_text_picks_by_luminance() {
        // Near-white fill → dark text; near-black fill → light text.
        assert!(ToastKind::prefers_dark_text(egui::Color32::from_gray(240)));
        assert!(!ToastKind::prefers_dark_text(egui::Color32::from_gray(20)));
    }

    #[test]
    fn background_is_neutral_opaque_window_fill() {
        // Issue #433 redesign: the pill fill is the theme's neutral, fully
        // opaque `window_fill` — the same for every kind (the kind shows in
        // the accent bar / icon, not the whole background). It must NOT be a
        // saturated per-kind blend, and it must NOT be translucent even when
        // `panel_fill` (the terminal-area fill) is translucent.
        let mut v = egui::Visuals::dark();
        v.error_fg_color = egui::Color32::from_rgb(200, 0, 0);
        v.warn_fg_color = egui::Color32::from_rgb(0, 200, 0);
        v.hyperlink_color = egui::Color32::from_rgb(0, 0, 200);
        v.window_fill = egui::Color32::from_rgb(30, 32, 36);
        // A translucent panel fill must not leak into the toast background.
        v.panel_fill = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 128);

        let bg = ToastKind::background(&v);

        assert_eq!(bg.a(), 255, "toast fill must be fully opaque");
        assert_eq!(
            (bg.r(), bg.g(), bg.b()),
            (30, 32, 36),
            "toast fill is the neutral window_fill, not a per-kind tint"
        );
    }

    #[test]
    fn push_error_appears_in_stack() {
        let mut s = ToastStack::default();
        s.error("spawn failed", Some("no such file".to_owned()));
        assert_eq!(s.entries.len(), 1);
        assert_eq!(s.entries[0].kind, ToastKind::Error);
        assert_eq!(s.entries[0].title, "spawn failed");
        assert_eq!(s.entries[0].detail.as_deref(), Some("no such file"));
    }

    // -----------------------------------------------------------------
    //  ToastStack::push_positioned — Warning severity
    // -----------------------------------------------------------------

    #[test]
    fn push_positioned_warning_pushes_warning_kind() {
        let mut s = ToastStack::default();
        s.push_positioned(
            ToastKind::Warning,
            "paste blocked",
            None,
            ToastPlacement::WINDOW_CENTERED,
        );
        assert_eq!(s.len(), 1);
        assert_eq!(s.last_kind(), Some(ToastKind::Warning));
    }

    #[test]
    fn push_positioned_stores_the_given_placement() {
        // Regression (adversarial review, slice 2): the placement passed to
        // `push_positioned` must survive `push` -> `Toast::new` onto the
        // stored `Toast`. Without this assertion, silently dropping the
        // placement and always storing `TOP_RIGHT` — the exact hardcoded
        // regression this slice removes — would go undetected.
        let mut s = ToastStack::default();
        s.push_positioned(
            ToastKind::Info,
            "copied",
            None,
            ToastPlacement::WINDOW_CENTERED,
        );
        assert_eq!(s.entries[0].position(), ToastPosition::WindowCentered);
        assert_eq!(s.entries[0].origin(), None);
    }

    #[test]
    fn error_and_info_wrappers_default_to_top_right() {
        // The severity convenience pushers must remain top-right stacked
        // (unchanged pre-existing behavior for error/info/OSC toasts).
        let mut s = ToastStack::default();
        s.error("boom", None);
        s.info("fyi", None);
        assert_eq!(s.entries[0].position(), ToastPosition::TopRightStack);
        assert_eq!(s.entries[0].origin(), None);
        assert_eq!(s.entries[1].position(), ToastPosition::TopRightStack);
        assert_eq!(s.entries[1].origin(), None);
    }

    #[test]
    fn stack_evicts_oldest_past_cap() {
        let mut s = ToastStack::default();
        for i in 0..(MAX_TOASTS + 3) {
            s.error(format!("err {i}"), None);
        }
        assert_eq!(s.entries.len(), MAX_TOASTS);
        // Oldest three should have been evicted; first surviving title is "err 3".
        assert_eq!(s.entries[0].title, "err 3");
    }

    #[test]
    fn toast_ids_are_monotonic() {
        let mut s = ToastStack::default();
        s.error("a", None);
        s.error("b", None);
        s.error("c", None);
        assert!(s.entries[0].id < s.entries[1].id);
        assert!(s.entries[1].id < s.entries[2].id);
    }

    #[test]
    fn expired_toast_is_detected() {
        // Drive expiry by synthesising a `now` that is 1 minute after the
        // toast's creation time, avoiding `Instant` subtraction which
        // clippy flags as unchecked.
        let created = Instant::now();
        let toast = Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            buttons: Vec::new(),
            icon: None,
            id: 0,
            created,
            life_start: created,
            lifetime: ToastLifetime::KindDefault,
            key: None,
            last_hovered: None,
            placement: ToastPlacement::TOP_RIGHT,
        };
        let later = created + Duration::from_mins(1);
        assert!(toast.is_expired(later));
    }

    #[test]
    fn hovered_toast_is_not_expired() {
        let created = Instant::now();
        let later = created + Duration::from_mins(1);
        let toast = Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            buttons: Vec::new(),
            icon: None,
            id: 0,
            created,
            life_start: created,
            lifetime: ToastLifetime::KindDefault,
            key: None,
            // Hover event coincides with `later` — within the 200 ms
            // keep-alive window.
            last_hovered: Some(later),
            placement: ToastPlacement::TOP_RIGHT,
        };
        assert!(!toast.is_expired(later));
    }

    #[test]
    fn stale_hover_does_not_preserve_toast() {
        let created = Instant::now();
        let stale_hover = created + Duration::from_secs(55);
        let later = created + Duration::from_mins(1);
        let toast = Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            buttons: Vec::new(),
            icon: None,
            id: 0,
            created,
            life_start: created,
            lifetime: ToastLifetime::KindDefault,
            key: None,
            // Hover was 5 s ago — well past HOVER_HOLD + ANIM_OUT.
            last_hovered: Some(stale_hover),
            placement: ToastPlacement::TOP_RIGHT,
        };
        assert!(toast.is_expired(later));
    }

    // -----------------------------------------------------------------
    //  Toast::anim — entry / hold / exit phases
    // -----------------------------------------------------------------

    fn anim_test_toast(created: Instant, last_hovered: Option<Instant>) -> Toast {
        Toast {
            kind: ToastKind::Info,
            title: "t".to_owned(),
            detail: None,
            buttons: Vec::new(),
            icon: None,
            id: 0,
            created,
            life_start: created,
            lifetime: ToastLifetime::KindDefault,
            key: None,
            last_hovered,
            placement: ToastPlacement::TOP_RIGHT,
        }
    }

    #[test]
    fn anim_at_creation_is_fully_entry() {
        let created = Instant::now();
        let toast = anim_test_toast(created, None);
        let anim = toast.anim(created);
        assert!(
            anim.opacity.abs() < f32::EPSILON,
            "opacity={}",
            anim.opacity
        );
        assert!(
            (anim.slide_x - SLIDE_IN_PTS).abs() < f32::EPSILON,
            "slide_x={}",
            anim.slide_x
        );
        assert!(
            (anim.scale - SCALE_IN).abs() < f32::EPSILON,
            "scale={}",
            anim.scale
        );
    }

    #[test]
    fn anim_mid_entry_is_between_start_and_settled() {
        let created = Instant::now();
        let toast = anim_test_toast(created, None);
        let anim = toast.anim(created + Duration::from_millis(90));
        assert!(anim.opacity > 0.0 && anim.opacity < 1.0, "{}", anim.opacity);
        assert!(
            anim.slide_x > 0.0 && anim.slide_x < SLIDE_IN_PTS,
            "{}",
            anim.slide_x
        );
        assert!(anim.scale > SCALE_IN && anim.scale < 1.0, "{}", anim.scale);
    }

    #[test]
    fn anim_settled_hold_is_fully_visible_and_unslid() {
        let created = Instant::now();
        let toast = anim_test_toast(created, None);
        // Well past entry (ANIM_IN), well before the last ANIM_OUT of the
        // default duration.
        let hold_point = ANIM_IN
            + toast
                .kind
                .default_duration()
                .saturating_sub(ANIM_IN)
                .saturating_sub(ANIM_OUT)
                / 2;
        let anim = toast.anim(created + hold_point);
        assert!((anim.opacity - 1.0).abs() < f32::EPSILON);
        assert!(anim.slide_x.abs() < f32::EPSILON);
        assert!((anim.scale - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn anim_exit_fades_out_over_the_last_slice() {
        let created = Instant::now();
        let toast = anim_test_toast(created, None);
        // 100ms of the lifetime remaining -> inside the ANIM_OUT exit window.
        let now = created
            + toast
                .kind
                .default_duration()
                .saturating_sub(Duration::from_millis(100));
        let anim = toast.anim(now);
        // remaining (100ms) / ANIM_OUT, both in the same units.
        let expected_q = 0.100 / ANIM_OUT.as_secs_f32();
        assert!(
            (anim.opacity - expected_q).abs() < 0.01,
            "opacity={} expected~{expected_q}",
            anim.opacity
        );
        assert!(anim.slide_x.abs() < f32::EPSILON, "exit does not re-slide");
        assert!(
            (anim.scale - 1.0).abs() < f32::EPSILON,
            "exit does not re-scale"
        );
    }

    #[test]
    fn anim_hover_suppresses_exit_fade() {
        let created = Instant::now();
        // Hovered right up until `now` -> within the HOVER_HOLD grace.
        let now = created
            + ToastKind::Info
                .default_duration()
                .saturating_sub(Duration::from_millis(50));
        let toast = anim_test_toast(created, Some(now));
        let anim = toast.anim(now);
        assert!(
            (anim.opacity - 1.0).abs() < f32::EPSILON,
            "hover must keep the toast fully opaque: {}",
            anim.opacity
        );
    }

    #[test]
    fn anim_fades_out_after_hover_extends_past_nominal_life() {
        // Review finding #2: a toast hovered past its nominal duration must
        // still fade out over ANIM_OUT once the pointer leaves — not pop.
        let created = Instant::now();
        // Last hovered well past the 6s Info life (so age > total), and the
        // hover ended long enough ago to be past HOVER_HOLD but partway into
        // the ANIM_OUT fade.
        let total = ToastKind::Info.default_duration();
        let hover_end = created + total + Duration::from_secs(5);
        let toast = anim_test_toast(created, Some(hover_end));

        // Just after HOVER_HOLD ends: nearly fully opaque, fading.
        let early = hover_end + HOVER_HOLD + Duration::from_millis(20);
        let a_early = toast.anim(early);
        assert!(
            a_early.opacity > 0.8 && a_early.opacity < 1.0,
            "fade just started: {}",
            a_early.opacity
        );
        assert!(!toast.is_expired(early), "must still be alive while fading");

        // Near the end of the fade window: nearly transparent.
        let fade_span =
            HOVER_HOLD.saturating_add(ANIM_OUT.saturating_sub(Duration::from_millis(20)));
        let late = hover_end + fade_span;
        let a_late = toast.anim(late);
        assert!(
            a_late.opacity < 0.2,
            "fade nearly complete: {}",
            a_late.opacity
        );

        // Opacity decreases monotonically as the fade progresses.
        assert!(
            a_late.opacity < a_early.opacity,
            "fade progresses: early={} late={}",
            a_early.opacity,
            a_late.opacity
        );

        // After the full fade window, it is removed.
        let done = hover_end + HOVER_HOLD + ANIM_OUT + Duration::from_millis(10);
        assert!(toast.is_expired(done), "removed only after the fade");
    }

    // -----------------------------------------------------------------
    //  ToastKind::icon / semantic
    // -----------------------------------------------------------------

    #[test]
    fn icon_maps_error_and_warning_to_warning_glyph() {
        assert_eq!(ToastKind::Error.icon(), ChromeIcon::Warning);
        assert_eq!(ToastKind::Warning.icon(), ChromeIcon::Warning);
    }

    #[test]
    fn icon_maps_info_to_bell_glyph() {
        assert_eq!(ToastKind::Info.icon(), ChromeIcon::Bell);
    }

    // -----------------------------------------------------------------
    //  Keyed toasts: lifecycle, events, lifetimes
    // -----------------------------------------------------------------

    const NO_EVENTS: [ToastEvent; 0] = [];

    fn push_keyed_info(s: &mut ToastStack, key: u64, lifetime: ToastLifetime) {
        s.push_keyed(
            ToastKey(key),
            info_content(&format!("toast {key}")),
            lifetime,
            ToastPlacement::TOP_RIGHT,
        );
    }

    fn info_content(title: &str) -> ToastContent {
        ToastContent {
            kind: ToastKind::Info,
            title: title.to_owned(),
            detail: None,
            buttons: Vec::new(),
            icon: None,
        }
    }

    fn closed(key: u64) -> ToastEvent {
        ToastEvent {
            key: ToastKey(key),
            kind: ToastEventKind::Closed,
        }
    }

    fn activated(key: u64) -> ToastEvent {
        ToastEvent {
            key: ToastKey(key),
            kind: ToastEventKind::Activated,
        }
    }

    #[test]
    fn push_keyed_then_close_keyed_emits_exactly_one_closed() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        assert_eq!(s.len(), 1);
        assert_eq!(s.take_events(), NO_EVENTS, "push itself emits nothing");

        s.close_keyed(ToastKey(1));
        assert!(s.is_empty());
        assert_eq!(s.take_events(), vec![closed(1)]);
        assert_eq!(s.take_events(), NO_EVENTS, "take_events drains");
    }

    #[test]
    fn close_keyed_of_an_absent_key_is_a_no_op() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        s.info("unkeyed", None);
        s.close_keyed(ToastKey(99));
        assert_eq!(s.len(), 2);
        assert_eq!(s.take_events(), NO_EVENTS);
    }

    #[test]
    fn closing_an_unkeyed_toast_never_emits() {
        let mut s = ToastStack::default();
        s.info("unkeyed", None);
        let id = s.entries[0].id;
        s.apply_click(id, ToastClickPart::Body);
        assert!(s.is_empty());
        assert_eq!(s.take_events(), NO_EVENTS);
    }

    #[test]
    fn keyed_fixed_toast_expiry_records_closed_on_sweep() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 7, ToastLifetime::Fixed(Duration::from_secs(2)));
        s.info("unkeyed", None);
        let created = s.entries[0].created;

        s.sweep_expired(created + Duration::from_secs(1));
        assert_eq!(s.len(), 2, "not yet expired");
        assert_eq!(s.take_events(), NO_EVENTS);

        // Past the lifetime (and its trailing fade window is not extended,
        // since it was never hovered).
        s.sweep_expired(created + Duration::from_millis(2001));
        assert_eq!(s.len(), 1, "only the keyed Fixed(2s) toast expired");
        assert_eq!(s.take_events(), vec![closed(7)]);
    }

    #[test]
    fn until_dismissed_never_expires() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::UntilDismissed);
        let created = s.entries[0].created;
        let far = created + Duration::from_hours(24 * 365);
        assert!(!s.entries[0].is_expired(far));
        s.sweep_expired(far);
        assert_eq!(s.len(), 1);
        assert_eq!(s.take_events(), NO_EVENTS);
        // And it never fades: it is fully settled forever after its entry.
        assert_eq!(s.entries[0].anim(far), SETTLED_ANIM);
    }

    #[test]
    fn until_dismissed_still_closes_by_click_and_close_keyed() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::UntilDismissed);
        s.close_keyed(ToastKey(1));
        assert_eq!(s.take_events(), vec![closed(1)]);
    }

    #[test]
    fn kind_default_keyed_toast_expires_like_an_unkeyed_one() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        s.info("unkeyed", None);
        let created = s.entries[0].created;
        let total = ToastKind::Info.default_duration();

        for toast in &s.entries {
            assert!(!toast.is_expired(created + total.saturating_sub(Duration::from_millis(1))));
            assert!(toast.is_expired(created + total + Duration::from_millis(1)));
        }
        s.sweep_expired(created + total + Duration::from_millis(1));
        assert!(s.is_empty());
        assert_eq!(s.take_events(), vec![closed(1)], "unkeyed emits nothing");
    }

    #[test]
    fn fixed_lifetime_anim_fades_in_lockstep_with_expiry() {
        // The exit fade must use the same total as `is_expired`.
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::Fixed(Duration::from_secs(60)));
        let toast = &s.entries[0];
        let created = toast.created;
        // Well inside a 60s life (an Info default would already be gone).
        let mid = created + Duration::from_secs(30);
        assert!(!toast.is_expired(mid));
        assert_eq!(toast.anim(mid), SETTLED_ANIM);
        // Last 100ms of the 60s life: fading, still alive.
        let fading = created + Duration::from_millis(59_900);
        assert!(!toast.is_expired(fading));
        assert!(toast.anim(fading).opacity < 1.0);
        assert!(toast.is_expired(created + Duration::from_millis(60_001)));
    }

    #[test]
    fn evicting_past_max_toasts_closes_only_the_first_keyed() {
        let mut s = ToastStack::default();
        for key in 0..=u64::try_from(MAX_TOASTS).unwrap() {
            push_keyed_info(&mut s, key, ToastLifetime::KindDefault);
        }
        assert_eq!(s.len(), MAX_TOASTS);
        assert_eq!(s.take_events(), vec![closed(0)]);
    }

    #[test]
    fn evicting_an_unkeyed_toast_emits_nothing() {
        let mut s = ToastStack::default();
        s.info("old unkeyed", None);
        for key in 0..u64::try_from(MAX_TOASTS).unwrap() {
            push_keyed_info(&mut s, key, ToastLifetime::KindDefault);
        }
        assert_eq!(s.len(), MAX_TOASTS);
        assert_eq!(s.take_events(), NO_EVENTS);
    }

    #[test]
    fn replace_keyed_keeps_slot_placement_and_created_and_resets_life() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        s.push_keyed(
            ToastKey(2),
            info_content("second"),
            ToastLifetime::KindDefault,
            ToastPlacement::WINDOW_CENTERED,
        );
        push_keyed_info(&mut s, 3, ToastLifetime::KindDefault);
        s.take_events();
        let before = s.entries[1].clone();

        let later = before.created + Duration::from_secs(5);
        let outcome = s.replace_keyed_at(
            later,
            ToastKey(2),
            ToastKey(20),
            ToastContent {
                kind: ToastKind::Error,
                title: "updated".to_owned(),
                detail: Some("d".to_owned()),
                buttons: Vec::new(),
                icon: None,
            },
            ToastLifetime::Fixed(Duration::from_secs(9)),
        );
        assert_eq!(outcome, ToastReplace::Replaced);
        assert_eq!(s.len(), 3);

        let after = &s.entries[1];
        assert_eq!(after.id, before.id, "same slot");
        assert_eq!(after.placement, ToastPlacement::WINDOW_CENTERED);
        assert_eq!(after.created, before.created, "no re-entry animation");
        assert_eq!(after.life_start, later, "lifetime restarts");
        assert_eq!(after.key, Some(ToastKey(20)));
        assert_eq!(after.kind, ToastKind::Error);
        assert_eq!(after.title, "updated");
        assert_eq!(after.detail.as_deref(), Some("d"));
        assert_eq!(after.lifetime, ToastLifetime::Fixed(Duration::from_secs(9)));
        assert_eq!(
            s.take_events(),
            NO_EVENTS,
            "replace emits no event for the old key"
        );
        // The old key is gone, the new one is closable.
        s.close_keyed(ToastKey(2));
        assert_eq!(s.take_events(), NO_EVENTS);
        s.close_keyed(ToastKey(20));
        assert_eq!(s.take_events(), vec![closed(20)]);
    }

    #[test]
    fn replace_keyed_restarts_the_expiry_clock() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::Fixed(Duration::from_secs(10)));
        let created = s.entries[0].created;
        let replaced_at = created + Duration::from_secs(9);
        s.replace_keyed_at(
            replaced_at,
            ToastKey(1),
            ToastKey(1),
            info_content("u"),
            ToastLifetime::Fixed(Duration::from_secs(10)),
        );
        // 11s after creation would have expired the old clock.
        assert!(!s.entries[0].is_expired(created + Duration::from_secs(11)));
        assert!(s.entries[0].is_expired(replaced_at + Duration::from_millis(10_001)));
    }

    #[test]
    fn replace_keyed_with_an_absent_old_key_pushes() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        let outcome = s.replace_keyed(
            ToastKey(99),
            ToastKey(5),
            info_content("new"),
            ToastLifetime::UntilDismissed,
        );
        assert_eq!(outcome, ToastReplace::Pushed);
        assert_eq!(s.len(), 2);
        assert_eq!(s.entries[1].key, Some(ToastKey(5)));
        assert_eq!(s.entries[1].lifetime, ToastLifetime::UntilDismissed);
        assert_eq!(s.entries[1].placement, ToastPlacement::TOP_RIGHT);
        assert_eq!(s.take_events(), NO_EVENTS);
    }

    #[test]
    fn replace_keyed_public_wrapper_updates_in_place() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        let id = s.entries[0].id;
        let outcome = s.replace_keyed(
            ToastKey(1),
            ToastKey(2),
            ToastContent {
                kind: ToastKind::Warning,
                ..info_content("w")
            },
            ToastLifetime::KindDefault,
        );
        assert_eq!(outcome, ToastReplace::Replaced);
        assert_eq!(s.entries[0].id, id);
        assert_eq!(s.entries[0].key, Some(ToastKey(2)));
    }

    #[test]
    fn toast_icon_is_the_override_or_the_kind_icon() {
        let mut s = ToastStack::default();
        s.info("unkeyed", None);
        s.push_keyed(
            ToastKey(1),
            ToastContent {
                icon: Some(ChromeIcon::FileManager),
                ..info_content("override")
            },
            ToastLifetime::KindDefault,
            ToastPlacement::TOP_RIGHT,
        );
        push_keyed_info(&mut s, 2, ToastLifetime::KindDefault);
        assert_eq!(s.entries[0].icon(), ToastKind::Info.icon());
        assert_eq!(s.entries[1].icon(), ChromeIcon::FileManager);
        assert_eq!(s.entries[2].icon(), ToastKind::Info.icon());
        assert_eq!(
            s.entries[1].kind,
            ToastKind::Info,
            "the override does not change the kind (and so not the accent)"
        );
    }

    #[test]
    fn replace_keyed_swaps_and_clears_the_icon() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 1, ToastLifetime::KindDefault);
        let with_icon = |icon| ToastContent {
            icon,
            ..info_content("x")
        };
        s.replace_keyed(
            ToastKey(1),
            ToastKey(2),
            with_icon(Some(ChromeIcon::Help)),
            ToastLifetime::KindDefault,
        );
        assert_eq!(s.entries[0].icon(), ChromeIcon::Help);
        s.replace_keyed(
            ToastKey(2),
            ToastKey(3),
            with_icon(None),
            ToastLifetime::KindDefault,
        );
        assert_eq!(s.entries[0].icon(), ToastKind::Info.icon());
    }

    // -----------------------------------------------------------------
    //  Click handling: stack-level `apply_click` + pure `click_target`
    // -----------------------------------------------------------------

    #[test]
    fn unkeyed_click_removes_with_no_events_for_either_part() {
        for part in [ToastClickPart::Body, ToastClickPart::Dismiss] {
            let mut s = ToastStack::default();
            s.error("e", None);
            let id = s.entries[0].id;
            s.apply_click(id, part);
            assert!(s.is_empty());
            assert_eq!(s.take_events(), NO_EVENTS);
        }
    }

    #[test]
    fn keyed_body_click_is_activated_then_closed() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 4, ToastLifetime::KindDefault);
        let id = s.entries[0].id;
        s.apply_click(id, ToastClickPart::Body);
        assert!(s.is_empty());
        assert_eq!(s.take_events(), vec![activated(4), closed(4)]);
    }

    #[test]
    fn keyed_dismiss_click_is_closed_only() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 4, ToastLifetime::KindDefault);
        let id = s.entries[0].id;
        s.apply_click(id, ToastClickPart::Dismiss);
        assert!(s.is_empty());
        assert_eq!(s.take_events(), vec![closed(4)]);
    }

    #[test]
    fn click_on_an_absent_id_is_a_no_op() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 4, ToastLifetime::KindDefault);
        s.apply_click(12_345, ToastClickPart::Body);
        assert_eq!(s.len(), 1);
        assert_eq!(s.take_events(), NO_EVENTS);
    }

    #[test]
    fn keyed_button_click_is_button_n_then_closed() {
        let mut s = ToastStack::default();
        push_keyed_info(&mut s, 4, ToastLifetime::KindDefault);
        let id = s.entries[0].id;
        s.apply_click(id, ToastClickPart::Button(2));
        assert!(s.is_empty());
        let button = ToastEvent {
            key: ToastKey(4),
            kind: ToastEventKind::Button(2),
        };
        assert_eq!(s.take_events(), vec![button, closed(4)]);
    }

    #[test]
    fn unkeyed_toast_ignores_a_button_part() {
        let mut s = ToastStack::default();
        s.info("i", None);
        let id = s.entries[0].id;
        s.apply_click(id, ToastClickPart::Button(1));
        assert!(s.is_empty());
        assert_eq!(s.take_events(), NO_EVENTS);
    }

    #[test]
    fn more_than_eight_buttons_are_capped_to_eight() {
        let many: Vec<String> = (0..12).map(|i| format!("b{i}")).collect();
        let content = ToastContent {
            buttons: many.clone(),
            icon: None,
            ..info_content("t")
        };
        let mut s = ToastStack::default();
        s.push_keyed(
            ToastKey(1),
            content.clone(),
            ToastLifetime::KindDefault,
            ToastPlacement::TOP_RIGHT,
        );
        assert_eq!(s.entries[0].buttons, many[..8]);

        // The in-place replace caps too, and replaces the old buttons.
        s.replace_keyed(
            ToastKey(1),
            ToastKey(2),
            content,
            ToastLifetime::KindDefault,
        );
        assert_eq!(s.entries[0].buttons.len(), 8);
        s.replace_keyed(
            ToastKey(2),
            ToastKey(3),
            info_content("no buttons"),
            ToastLifetime::KindDefault,
        );
        assert_eq!(s.entries[0].buttons.len(), 0);
    }

    #[test]
    fn unkeyed_toasts_have_no_buttons() {
        let mut s = ToastStack::default();
        s.error("e", None);
        assert_eq!(s.entries[0].buttons.len(), 0);
    }
}
