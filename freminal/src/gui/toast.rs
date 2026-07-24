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
//! toast immediately by clicking anywhere on it; the "x" glyph shown at the
//! pill's right edge is a visual affordance for this, not a separate hit
//! target.
//!
//! Toasts are drawn by a fully-owned OpenGL pass (issue #433), not egui
//! widgets: [`ToastStack::show`] measures/shapes each toast's text on the
//! main thread (`renderer::toast_text_pass::ToastTextRenderer`), lays it out
//! via [`layout_toasts`], and hands the resulting pill quads and text runs
//! to a single `PaintCallback` that draws pills
//! (`renderer::toast_pass::ToastRenderer`) then text on top. Hover/dismiss
//! hit-testing is done in owned code against egui's pointer state, since
//! there are no interactive egui widgets to generate `Response`s.

use std::time::{Duration, Instant};

use super::font_manager::FontManager;
use super::icons::ChromeIcon;
use super::renderer::ToastRenderState;

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
    /// Warning that does not prevent continued operation.
    #[allow(dead_code)]
    // Reserved for future subtasks (71.3 layout non-fatal, 71.4 shader warnings).
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

    /// Accent/glow color for this kind, taken directly from the active
    /// theme's palette semantic colors (error/warn/link) rather than a
    /// hard-coded hue. Used both by [`Self::background`] (the egui-rendered
    /// bubble fill) and by the owned-renderer layout's glow/accent-bar
    /// colors ([`layout_toasts`]).
    const fn semantic(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Self::Error => visuals.error_fg_color,
            Self::Warning => visuals.warn_fg_color,
            Self::Info => visuals.hyperlink_color,
        }
    }

    /// Icon glyph shown alongside this kind's toast in the owned-renderer
    /// layout ([`layout_toasts`]).
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

    /// Background tint for the toast bubble, derived from the active theme.
    ///
    /// Each toast kind maps to a palette semantic color (error/warn/link) so
    /// the bubble follows the theme instead of hard-coded hues. The semantic
    /// color is darkened toward the panel background and rendered at high
    /// opacity so it reads as a saturated bubble over any background.
    fn background(self, visuals: &egui::Visuals) -> egui::Color32 {
        let accent = self.semantic(visuals);
        // Blend the accent toward the panel background for a deeper bubble, then
        // apply a high (but not full) opacity so it sits over the terminal.
        let base = visuals.panel_fill;
        let mix = |a: u8, b: u8| -> u8 {
            use conv2::ConvUtil;
            // 65% accent, 35% panel background.
            f32::from(b)
                .mul_add(0.35, f32::from(a) * 0.65)
                .round()
                .clamp(0.0, 255.0)
                .approx_as::<u8>()
                .unwrap_or(b)
        };
        egui::Color32::from_rgba_unmultiplied(
            mix(accent.r(), base.r()),
            mix(accent.g(), base.g()),
            mix(accent.b(), base.b()),
            240,
        )
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

/// A single toast entry in the stack.
#[derive(Debug, Clone)]
pub(super) struct Toast {
    kind: ToastKind,
    /// One-line headline (bold).
    title: String,
    /// Optional multi-line detail (wrapped).
    detail: Option<String>,
    /// Monotonic id used as the egui widget id seed, so each toast has
    /// a stable id across frames even after list reordering.
    id: u64,
    /// When the toast was created.  Used with `default_duration()` to
    /// compute expiry (unless hovered).
    created: Instant,
    /// When the toast was last hovered.  `None` if never hovered.  Used
    /// to extend the lifetime of a toast the user is reading.
    last_hovered: Option<Instant>,
}

impl Toast {
    fn new(kind: ToastKind, title: String, detail: Option<String>, id: u64) -> Self {
        Self {
            kind,
            title,
            detail,
            id,
            created: Instant::now(),
            last_hovered: None,
        }
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
        // While actively hovered (within the hold grace), never expire.
        if self.hovered_recently(now) {
            return false;
        }
        let age = now.saturating_duration_since(self.created);
        let total = self.kind.default_duration();
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

        let age = now.saturating_duration_since(self.created);
        let total = self.kind.default_duration();

        // Degenerate timelines (not expected in practice — every
        // `ToastKind::default_duration()` is nonzero — but guards against
        // div-by-zero if the animation durations are ever misconfigured).
        if ANIM_IN.is_zero() || ANIM_OUT.is_zero() || total.is_zero() {
            return SETTLED;
        }

        // Phase 1: entry.
        if age < ANIM_IN {
            let p = (age.as_secs_f32() / ANIM_IN.as_secs_f32()).clamp(0.0, 1.0);
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

// ---------------------------------------------------------------------------
//  Owned-renderer layout model (issue #433)
// ---------------------------------------------------------------------------
//
// The types and function below turn a list of toasts (plus their measured
// text and per-toast animation phase) into the GPU-facing draw data for the
// fully-owned toast renderer (`super::renderer::toast_pass` /
// `toast_text_pass`): one `ToastQuad` per pill and a handful of
// `ToastTextRun`s (icon/label/detail) per toast, laid out and animated in
// the *same* pass so the pill and its text can never drift apart. Hit
// rectangles for hover/dismiss are also computed here, in logical points
// (egui's own coordinate space), so [`ToastStack::show`] can hit-test
// pointer input against them without recomputing geometry.
//
// This module is pure and GL-free — no `FontManager`, no `glow::Context` —
// so it is fully unit-testable with synthesized `ToastTextMetrics`. It is
// wired into the frame by [`ToastStack::show`], which measures/shapes text
// on the main thread, calls `layout_toasts`, and hands the resulting
// `ToastQuad`/`ToastTextRun` data to a single `PaintCallback`.

use super::renderer::{ToastQuad, ToastTextMetrics, ToastTextRun};

/// Inner padding between a pill's edge and its content, logical points.
const PILL_PAD_X_PTS: f32 = 14.0;
/// Inner padding between a pill's top/bottom edge and its content, logical points.
const PILL_PAD_Y_PTS: f32 = 10.0;
/// Gap between the icon glyph and the label/detail column, logical points.
const ICON_LABEL_GAP_PTS: f32 = 8.0;
/// Gap between the label/detail column and the dismiss button, logical points.
const LABEL_CLOSE_GAP_PTS: f32 = 8.0;
/// Side length of the (square) dismiss button, logical points.
const CLOSE_SIZE_PTS: f32 = 18.0;
/// Vertical gap between successive toasts in the stack, logical points.
const TOAST_STACK_GAP_PTS: f32 = 8.0;
/// Gap between the label line and the detail line, logical points.
const DETAIL_GAP_PTS: f32 = 4.0;
/// Maximum pill width, logical points. Content wider than this clamps.
const MAX_PILL_WIDTH_PTS: f32 = 360.0;
/// Minimum pill height, logical points, regardless of content.
const MIN_PILL_HEIGHT_PTS: f32 = 40.0;
/// Floor applied to the theme's `menu_corner_radius`, logical points.
const MIN_CORNER_RADIUS_PTS: f32 = 8.0;
/// Inset from the window's right edge to the stack's right edge, logical points.
const STACK_RIGHT_INSET_PTS: f32 = 12.0;
/// Inset from the window's top edge to the stack's first toast, logical points.
const STACK_TOP_INSET_PTS: f32 = 44.0;

/// Where the toast stack is anchored within the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ToastPosition {
    /// Stacked in the top-right corner of the window — the only placement
    /// [`ToastStack::show`] currently produces toasts at (`Toast` carries no
    /// per-toast position yet).
    #[default]
    TopRightStack,
    /// The whole stack centered within the window.
    #[allow(dead_code)]
    // Reserved: no caller currently attaches a per-toast position (`Toast`
    // has no `position` field yet), so only `TopRightStack` is ever
    // constructed. Kept for a future toast kind that centers itself (e.g. a
    // blocking-confirmation-style toast).
    WindowCentered,
    /// The whole stack centered within the active pane. Falls back to the
    /// window rect when no active pane rect is known (e.g. no pane focused,
    /// or a window with no panes yet).
    #[allow(dead_code)]
    // Reserved: see `WindowCentered`.
    PaneCentered,
}

/// Geometry inputs to [`layout_toasts`], in LOGICAL points (egui's own
/// coordinate space — the same space `ctx.content_rect()` and pane layout
/// rects live in).
#[derive(Debug, Clone, Copy)]
pub(super) struct ToastGeometry {
    /// The full window rect.
    pub window_rect: egui::Rect,
    /// The currently-active pane's rect, if known. Consulted only by
    /// [`ToastPosition::PaneCentered`].
    pub active_pane_rect: Option<egui::Rect>,
}

/// Per-toast input to [`layout_toasts`]: its measured content plus its
/// current animation phase.
///
/// Carries the exact text (`label_text`/`detail_text`, needed to build the
/// actual [`ToastTextRun`]s), the physical pixel size each was measured at
/// (`label_size_px`/`detail_size_px`/`icon_size_px` — needed so the run
/// handed to [`super::renderer::ToastTextRenderer::build_instances`] later
/// rasterizes at *exactly* the size [`ToastTextMetrics`] was measured at;
/// any mismatch would desync the pill's width from the glyphs actually
/// drawn), and each string's pre-measured [`ToastTextMetrics`] (needed for
/// layout math). The caller measures text exactly once (via
/// [`super::renderer::ToastTextRenderer::measure`]) and this function never
/// re-shapes it.
#[derive(Debug, Clone)]
pub(super) struct ToastLayoutInput {
    /// Stable id, matching [`Toast::id`], used to key the returned outputs
    /// back to their source toast (e.g. for hit-testing dismiss clicks).
    pub id: u64,
    /// The toast's severity — drives color and icon.
    pub kind: ToastKind,
    /// The exact `"Kind: title"` label string that was measured.
    pub label_text: String,
    /// The physical pixel size `label_text` was measured/rasterized at.
    pub label_size_px: f32,
    /// Measured `label_text`, in **physical** pixels (see [`ToastTextMetrics`]).
    pub label: ToastTextMetrics,
    /// The exact detail string that was measured. Empty when `has_detail`
    /// is `false`.
    pub detail_text: String,
    /// The physical pixel size `detail_text` was measured/rasterized at.
    pub detail_size_px: f32,
    /// Measured `detail_text`, in physical pixels. Zero-valued when
    /// `has_detail` is `false`.
    pub detail: ToastTextMetrics,
    /// Whether this toast has a detail line to render.
    pub has_detail: bool,
    /// The physical pixel size the icon glyph was measured/rasterized at.
    pub icon_size_px: f32,
    /// Measured icon glyph (a single glyph), in physical pixels.
    pub icon: ToastTextMetrics,
    /// The physical pixel size the dismiss ("x") glyph was
    /// measured/rasterized at.
    pub close_size_px: f32,
    /// Measured dismiss ("x") glyph, in physical pixels.
    pub close: ToastTextMetrics,
    /// This toast's current animation phase (see [`Toast::anim`]).
    pub anim: ToastAnim,
}

/// One laid-out toast, ready to hand to the toast/toast-text GPU passes.
///
/// All fields describing GPU draw data (`pill`, `runs`) are in **physical**
/// pixels, viewport-local to the toast overlay's `PaintCallback` rect
/// (origin = top-left of the callback's clip rect) — matching [`ToastQuad`]
/// and [`ToastTextRun`]'s own documented coordinate space. The hit-test
/// rects are in **logical** points (window space) since that is the space
/// egui's own pointer/hover queries operate in.
#[derive(Debug, Clone)]
pub(super) struct ToastLayoutOutput {
    /// Matches the source [`ToastLayoutInput::id`].
    pub id: u64,
    /// The pill background quad.
    pub pill: ToastQuad,
    /// Text runs to draw for this toast: icon, label, and (if present)
    /// detail, in that order.
    pub runs: Vec<ToastTextRun>,
    /// Interactable pill rect, logical points, window space:
    /// `[min_x, min_y, max_x, max_y]`. The whole pill is the click target —
    /// a primary click anywhere within this rect dismisses the toast (see
    /// [`ToastStack::hit_test`]); the drawn "x" glyph (in `runs`) is purely
    /// a visual affordance, not a separate hit region.
    pub hit_rect_logical: [f32; 4],
}

/// The maximum corner radius (in logical points) among the theme's
/// `menu_corner_radius` four corners, floored at [`MIN_CORNER_RADIUS_PTS`].
fn corner_radius_pts(visuals: &egui::Visuals) -> f32 {
    let corner = visuals.menu_corner_radius;
    let max_corner = corner.nw.max(corner.ne).max(corner.sw).max(corner.se);
    f32::from(max_corner).max(MIN_CORNER_RADIUS_PTS)
}

/// Convert a straight-alpha [`egui::Color32`] to straight (non-premultiplied)
/// `[r, g, b, a]` floats in `0..=1`.
fn color32_to_rgba(color: egui::Color32) -> [f32; 4] {
    // `Color32`'s accessors (`r()`/`g()`/`b()`) return components
    // *premultiplied* by alpha (see the `ecolor::Color32` type docs) — this
    // pass's `ToastQuad`/`ToastTextRun` colors are documented as *straight*
    // RGBA, so the alpha must be un-premultiplied via `to_srgba_unmultiplied`
    // rather than reading the accessors directly.
    let straight = color.to_srgba_unmultiplied();
    [
        f32::from(straight[0]) / 255.0,
        f32::from(straight[1]) / 255.0,
        f32::from(straight[2]) / 255.0,
        f32::from(straight[3]) / 255.0,
    ]
}

/// Shift each color channel of `rgba` by `amount` (positive lightens,
/// negative darkens), clamped to `0..=1`. Alpha is left untouched.
fn lighten(rgba: [f32; 4], amount: f32) -> [f32; 4] {
    [
        (rgba[0] + amount).clamp(0.0, 1.0),
        (rgba[1] + amount).clamp(0.0, 1.0),
        (rgba[2] + amount).clamp(0.0, 1.0),
        rgba[3],
    ]
}

/// The settled (pre-animation) pill width/height, in physical pixels, for
/// one toast's measured content.
fn settled_pill_size(input: &ToastLayoutInput, ppp: f32) -> (f32, f32) {
    let pad_x = PILL_PAD_X_PTS * ppp;
    let pad_y = PILL_PAD_Y_PTS * ppp;
    let icon_label_gap = ICON_LABEL_GAP_PTS * ppp;
    let label_close_gap = LABEL_CLOSE_GAP_PTS * ppp;
    let close_size = CLOSE_SIZE_PTS * ppp;
    let detail_gap = DETAIL_GAP_PTS * ppp;
    let max_width = MAX_PILL_WIDTH_PTS * ppp;
    let min_height = MIN_PILL_HEIGHT_PTS * ppp;

    let text_width = input.label.width.max(input.detail.width);
    let content_width =
        input.icon.width + icon_label_gap + text_width + label_close_gap + close_size;
    let width = pad_x.mul_add(2.0, content_width).min(max_width);

    let mut text_height = input.label.height;
    if input.has_detail {
        text_height += detail_gap + input.detail.height;
    }
    let height = pad_y.mul_add(2.0, text_height).max(min_height);

    (width, height)
}

/// Compute the settled (pre-animation) top-left origin, in physical pixels,
/// of each toast's pill, for the given anchor `position`.
///
/// `sizes[i]` is `(width, height)` for `inputs[i]`, as returned by
/// [`settled_pill_size`]. Returns one `(x, y)` pair per input, in the same
/// order.
fn settled_origins(
    sizes: &[(f32, f32)],
    position: ToastPosition,
    geom: ToastGeometry,
    ppp: f32,
) -> Vec<(f32, f32)> {
    let gap = TOAST_STACK_GAP_PTS * ppp;

    // Cumulative vertical offset of each toast's top edge, relative to the
    // stack's own top (offset 0 for the first toast).
    let mut offsets = Vec::with_capacity(sizes.len());
    let mut cumulative = 0.0;
    for &(_, height) in sizes {
        offsets.push(cumulative);
        cumulative += height + gap;
    }
    let total_height = if sizes.is_empty() {
        0.0
    } else {
        cumulative - gap
    };

    let rect_for = |rect: egui::Rect| {
        (
            rect.min.x * ppp,
            rect.min.y * ppp,
            rect.max.x * ppp,
            rect.max.y * ppp,
        )
    };

    match position {
        ToastPosition::TopRightStack => {
            let (_, win_min_y, win_max_x, _) = rect_for(geom.window_rect);
            let base_y = STACK_TOP_INSET_PTS.mul_add(ppp, win_min_y);
            let right_edge = STACK_RIGHT_INSET_PTS.mul_add(-ppp, win_max_x);
            sizes
                .iter()
                .zip(&offsets)
                .map(|(&(width, _), &offset)| (right_edge - width, base_y + offset))
                .collect()
        }
        ToastPosition::WindowCentered => {
            let (win_min_x, win_min_y, win_max_x, win_max_y) = rect_for(geom.window_rect);
            let center_x = win_min_x.midpoint(win_max_x);
            let base_y = win_min_y.midpoint(win_max_y) - total_height / 2.0;
            sizes
                .iter()
                .zip(&offsets)
                .map(|(&(width, _), &offset)| (center_x - width / 2.0, base_y + offset))
                .collect()
        }
        ToastPosition::PaneCentered => {
            let pane_rect = geom.active_pane_rect.unwrap_or(geom.window_rect);
            let (pane_min_x, pane_min_y, pane_max_x, pane_max_y) = rect_for(pane_rect);
            let center_x = pane_min_x.midpoint(pane_max_x);
            let base_y = pane_min_y.midpoint(pane_max_y) - total_height / 2.0;
            sizes
                .iter()
                .zip(&offsets)
                .map(|(&(width, _), &offset)| (center_x - width / 2.0, base_y + offset))
                .collect()
        }
    }
}

/// Precomputed, `pixels_per_point`-scaled layout constants shared across
/// every toast in one [`layout_toasts`] call. Bundled into one struct so the
/// per-toast helper functions stay under the `too_many_arguments` limit.
struct ToastLayoutMetrics {
    /// `pixels_per_point`, kept alongside the already-scaled fields below
    /// since logical-to-physical hit-rect conversion still needs it raw.
    ppp: f32,
    /// [`PILL_PAD_X_PTS`], scaled to physical pixels.
    pad_x: f32,
    /// [`PILL_PAD_Y_PTS`], scaled to physical pixels.
    pad_y: f32,
    /// [`ICON_LABEL_GAP_PTS`], scaled to physical pixels.
    icon_label_gap: f32,
    /// [`CLOSE_SIZE_PTS`], scaled to physical pixels.
    close_size: f32,
    /// [`DETAIL_GAP_PTS`], scaled to physical pixels.
    detail_gap: f32,
    /// The theme's corner radius (see [`corner_radius_pts`]), scaled to
    /// physical pixels.
    corner_radius: f32,
}

/// The pill/glow/text colors for one toast, derived from its kind's
/// semantic theme color and its current animation opacity.
struct ToastColors {
    /// Pill background gradient, top stop.
    color_top: [f32; 4],
    /// Pill background gradient, bottom stop.
    color_bottom: [f32; 4],
    /// Outer glow color (rgb) + intensity (a).
    glow: [f32; 4],
    /// Left accent-bar color.
    accent: [f32; 4],
    /// Label/detail text color, contrast-picked against the pill fill.
    text_color: [f32; 4],
    /// Icon glyph color (tinted to the kind's semantic color).
    icon_color: [f32; 4],
}

/// Derive [`ToastColors`] for `kind` from the active theme, at the given
/// (already-clamped-elsewhere) animation `opacity`.
fn toast_colors(kind: ToastKind, visuals: &egui::Visuals, opacity: f32) -> ToastColors {
    let background = kind.background(visuals);
    let base_rgba = color32_to_rgba(background);
    let color_top = lighten(base_rgba, 0.08);
    let color_bottom = lighten(base_rgba, -0.06);
    let semantic_rgba = color32_to_rgba(kind.semantic(visuals));
    let glow = [semantic_rgba[0], semantic_rgba[1], semantic_rgba[2], 0.35];
    let accent = [semantic_rgba[0], semantic_rgba[1], semantic_rgba[2], 1.0];

    let text_alpha = opacity.clamp(0.0, 1.0);
    let text_rgb = if ToastKind::prefers_dark_text(background) {
        [0.08, 0.08, 0.08]
    } else {
        [0.96, 0.96, 0.96]
    };
    let text_color = [text_rgb[0], text_rgb[1], text_rgb[2], text_alpha];
    let icon_color = [
        semantic_rgba[0],
        semantic_rgba[1],
        semantic_rgba[2],
        text_alpha,
    ];

    ToastColors {
        color_top,
        color_bottom,
        glow,
        accent,
        text_color,
        icon_color,
    }
}

/// Build the icon/label/(optional detail)/dismiss [`ToastTextRun`]s for one
/// toast, anchored to the pill's already-animated top-left corner `(x, y)`
/// and sized `width` x `height` (physical pixels, post-animation scale —
/// the same rect the pill quad itself occupies, so the dismiss glyph never
/// drifts from the pill under the scale-in animation).
fn build_text_runs(
    input: &ToastLayoutInput,
    x: f32,
    y: f32,
    width: f32,
    metrics: &ToastLayoutMetrics,
    colors: &ToastColors,
) -> Vec<ToastTextRun> {
    let icon_origin_x = x + metrics.pad_x;
    let icon_baseline_y = y + metrics.pad_y + input.icon.ascent;
    let mut runs = vec![ToastTextRun {
        text: input.kind.icon().glyph(),
        origin_x: icon_origin_x,
        baseline_y: icon_baseline_y,
        size_px: input.icon_size_px,
        color: colors.icon_color,
    }];

    let label_origin_x = icon_origin_x + input.icon.width + metrics.icon_label_gap;
    let label_baseline_y = y + metrics.pad_y + input.label.ascent;
    runs.push(ToastTextRun {
        text: input.label_text.clone(),
        origin_x: label_origin_x,
        baseline_y: label_baseline_y,
        size_px: input.label_size_px,
        color: colors.text_color,
    });

    if input.has_detail {
        let detail_origin_x = x + metrics.pad_x;
        let detail_baseline_y =
            y + metrics.pad_y + input.label.height + metrics.detail_gap + input.detail.ascent;
        runs.push(ToastTextRun {
            text: input.detail_text.clone(),
            origin_x: detail_origin_x,
            baseline_y: detail_baseline_y,
            size_px: input.detail_size_px,
            color: colors.text_color,
        });
    }

    // Dismiss ("x") glyph — purely a visual affordance (the whole pill is
    // the click target, see `ToastStack::hit_test`), drawn within the
    // reserved close-button region at the pill's right edge (physical
    // pixels, since text runs are physical).
    let close_right = x + width - metrics.pad_x;
    let close_left = close_right - metrics.close_size;
    let close_origin_x =
        (close_left + (metrics.close_size - input.close.width) / 2.0).max(close_left);
    // Share the label row's baseline so the "x" aligns with the title text
    // beside it (top-aligned header row), rather than centering in the full
    // pill height — which visually drops it too low, more so on a
    // two-line (detail) toast.
    let close_baseline_y = y + metrics.pad_y + input.label.ascent;
    // Dimmed relative to the label/detail text so it reads as a secondary
    // affordance rather than competing with the message; still respects the
    // animation's opacity via `colors.text_color`'s alpha.
    let close_color = [
        colors.text_color[0],
        colors.text_color[1],
        colors.text_color[2],
        colors.text_color[3] * 0.7,
    ];
    runs.push(ToastTextRun {
        text: ChromeIcon::Close.glyph(),
        origin_x: close_origin_x,
        baseline_y: close_baseline_y,
        size_px: input.close_size_px,
        color: close_color,
    });

    runs
}

/// Lay out one toast: apply its animation to the settled pill rect, derive
/// its colors, and build its text runs and hit rects.
fn layout_one_toast(
    input: &ToastLayoutInput,
    width: f32,
    height: f32,
    settled_x: f32,
    settled_y: f32,
    metrics: &ToastLayoutMetrics,
    visuals: &egui::Visuals,
) -> ToastLayoutOutput {
    let anim = input.anim;

    // Scale about the settled rect's center, then apply the horizontal
    // slide (converted from logical points to physical pixels) on top.
    let scaled_width = width * anim.scale;
    let scaled_height = height * anim.scale;
    let center_x = settled_x + width / 2.0;
    let center_y = settled_y + height / 2.0;
    let x = anim
        .slide_x
        .mul_add(metrics.ppp, center_x - scaled_width / 2.0);
    let y = center_y - scaled_height / 2.0;

    let colors = toast_colors(input.kind, visuals, anim.opacity);
    // Clamp the corner radius to half the pill's smaller dimension: a large
    // theme `menu_corner_radius` on a short pill could otherwise exceed the
    // half-extent and distort the SDF into a lozenge (review finding #6).
    let corner_radius = metrics
        .corner_radius
        .min(scaled_width.min(scaled_height) / 2.0)
        .max(0.0);
    let pill = ToastQuad {
        x,
        y,
        width: scaled_width,
        height: scaled_height,
        corner_radius,
        color_top: colors.color_top,
        color_bottom: colors.color_bottom,
        glow: colors.glow,
        accent: colors.accent,
        opacity: anim.opacity.clamp(0.0, 1.0),
    };

    let runs = build_text_runs(input, x, y, scaled_width, metrics, &colors);

    let hit_rect_logical = [
        x / metrics.ppp,
        y / metrics.ppp,
        (x + scaled_width) / metrics.ppp,
        (y + scaled_height) / metrics.ppp,
    ];

    ToastLayoutOutput {
        id: input.id,
        pill,
        runs,
        hit_rect_logical,
    }
}

/// Turn `inputs` into GPU-facing draw data: one [`ToastQuad`] pill and a
/// handful of [`ToastTextRun`]s per toast, plus logical-point hit rects.
///
/// Pure and GL-free (no `FontManager`, no `glow::Context`) — `inputs` already
/// carries pre-measured [`ToastTextMetrics`] and each toast's animation
/// phase, so this function only does geometry and color arithmetic.
pub(super) fn layout_toasts(
    inputs: &[ToastLayoutInput],
    position: ToastPosition,
    geom: ToastGeometry,
    visuals: &egui::Visuals,
    pixels_per_point: f32,
) -> Vec<ToastLayoutOutput> {
    if inputs.is_empty() {
        return Vec::new();
    }

    // Guard against a pathological `pixels_per_point` of 0 from the windowing
    // layer: `ppp` is used as a divisor when deriving the logical hit rect
    // (`layout_one_toast`), so a zero would yield `Inf`/`NaN` rects that never
    // match a pointer. Floor it at a tiny epsilon (review finding #5).
    let ppp = pixels_per_point.max(f32::EPSILON);
    let metrics = ToastLayoutMetrics {
        ppp,
        pad_x: PILL_PAD_X_PTS * ppp,
        pad_y: PILL_PAD_Y_PTS * ppp,
        icon_label_gap: ICON_LABEL_GAP_PTS * ppp,
        close_size: CLOSE_SIZE_PTS * ppp,
        detail_gap: DETAIL_GAP_PTS * ppp,
        corner_radius: corner_radius_pts(visuals) * ppp,
    };

    let sizes: Vec<(f32, f32)> = inputs.iter().map(|i| settled_pill_size(i, ppp)).collect();
    let origins = settled_origins(&sizes, position, geom, ppp);

    inputs
        .iter()
        .zip(&sizes)
        .zip(&origins)
        .map(|((input, &(width, height)), &(settled_x, settled_y))| {
            layout_one_toast(
                input, width, height, settled_x, settled_y, &metrics, visuals,
            )
        })
        .collect()
}

/// Ordered stack of active toasts, rendered top-to-bottom from the most
/// recent.  Capped at [`MAX_TOASTS`] entries; older entries are evicted
/// when the cap is exceeded.
#[derive(Debug, Default)]
pub(super) struct ToastStack {
    entries: Vec<Toast>,
    next_id: u64,
}

/// Maximum simultaneous toasts.  Older ones are evicted.
const MAX_TOASTS: usize = 5;

impl ToastStack {
    /// Push a new error toast onto the stack.
    pub(super) fn error(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(ToastKind::Error, title.into(), detail);
    }

    /// Push a new informational toast onto the stack.
    pub(super) fn info(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(ToastKind::Info, title.into(), detail);
    }

    /// Number of toasts currently on the stack. Test-only helper used by the
    /// notification router tests to assert toast-leg routing decisions.
    #[cfg(test)]
    pub(super) const fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the stack currently has no toasts. Used by the frame-damage
    /// aggregation (#435): a visible toast animates its own region each
    /// frame, so its presence forces a full-frame present.
    pub(super) const fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn push(&mut self, kind: ToastKind, title: String, detail: Option<String>) {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        self.entries.push(Toast::new(kind, title, detail, id));
        // Evict oldest entries beyond the cap.
        while self.entries.len() > MAX_TOASTS {
            self.entries.remove(0);
        }
    }

    /// Font size (physical pixels, pre-`pixels_per_point` scaling) the toast
    /// label is rasterized at. See [`Self::show`].
    const LABEL_SIZE_PTS: f32 = 14.0;
    /// Font size (physical pixels, pre-`pixels_per_point` scaling) the toast
    /// detail line is rasterized at. See [`Self::show`].
    const DETAIL_SIZE_PTS: f32 = 12.0;

    /// Render the stack as a fully-owned GL overlay (issue #433): measures
    /// and shapes every toast's text on the main thread, lays it out via
    /// [`layout_toasts`], and hands the result to a single `PaintCallback`
    /// that draws every pill, then every text run, on top of everything
    /// else in the window.
    ///
    /// `render_state` is this window's toast GL state (per-window, since GL
    /// resources belong to one GL context); `font_manager` is this window's
    /// shared [`FontManager`] (obtained via
    /// `FreminalTerminalWidget::font_manager_mut`), used only for
    /// measuring/shaping — never touched from inside the `PaintCallback`.
    ///
    /// Clears auto-expired toasts and any the user dismissed by clicking
    /// anywhere within a toast's pill (its `hit_rect_logical`, computed by
    /// `layout_toasts`).
    pub(super) fn show(
        &mut self,
        ctx: &egui::Context,
        geom: ToastGeometry,
        render_state: &std::sync::Arc<std::sync::Mutex<ToastRenderState>>,
        font_manager: &mut FontManager,
        pixels_per_point: f32,
    ) {
        if self.entries.is_empty() {
            return;
        }

        let now = Instant::now();
        let ppp = pixels_per_point;
        let visuals = ctx.global_style().visuals.clone();

        let label_size_px = Self::LABEL_SIZE_PTS * ppp;
        let detail_size_px = Self::DETAIL_SIZE_PTS * ppp;
        let icon_size_px = label_size_px;

        // Measure + shape every toast's text on the main thread; the lock is
        // held only for the duration of this call — no GL is touched.
        let (inputs, any_animating) = {
            let rs = render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.measure_inputs(
                &rs,
                font_manager,
                label_size_px,
                detail_size_px,
                icon_size_px,
                now,
            )
        };

        // `layout_toasts` emits window-origin-absolute physical-pixel
        // coordinates (it scales `geom.window_rect` directly, without
        // subtracting `window_rect.min`) — see its own field docs. The
        // `PaintCallback` registered by `paint_toasts` below uses
        // `rect: ctx.content_rect()` (the full window, origin `(0,0)`), so
        // its GL viewport starts at the window's own origin and these
        // coordinates need no further translation to land in the right
        // place.
        let outputs = layout_toasts(&inputs, ToastPosition::TopRightStack, geom, &visuals, ppp);

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
        let to_remove = self.hit_test(ctx, &outputs, now);

        let text_instances = {
            let mut rs = render_state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let runs: Vec<ToastTextRun> = outputs.iter().flat_map(|o| o.runs.clone()).collect();
            rs.text.build_instances(&runs, font_manager)
        };
        let pills: Vec<ToastQuad> = outputs.iter().map(|o| o.pill).collect();
        Self::paint_toasts(ctx, render_state, pills, text_instances);

        // Remove dismissed toasts.
        if !to_remove.is_empty() {
            self.entries.retain(|t| !to_remove.contains(&t.id));
        }
        // Remove expired toasts.
        self.entries.retain(|t| !t.is_expired(now));

        // Request a repaint soon so expiry/animation happens even without
        // input: a fast (16ms, ~60fps) cadence while any toast is animating
        // (entry/exit fade+slide+scale), otherwise a slow (250ms) cadence
        // just to catch expiry.
        if !self.entries.is_empty() {
            let delay = if any_animating {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(250)
            };
            ctx.request_repaint_after(delay);
        }
    }

    /// Measure and shape every active toast's text, returning the
    /// [`ToastLayoutInput`]s [`layout_toasts`] needs plus whether any toast
    /// is currently mid-animation (entry or exit). Split out of
    /// [`Self::show`] to keep it under the line-count limit.
    fn measure_inputs(
        &self,
        rs: &ToastRenderState,
        font_manager: &mut FontManager,
        label_size_px: f32,
        detail_size_px: f32,
        icon_size_px: f32,
        now: Instant,
    ) -> (Vec<ToastLayoutInput>, bool) {
        let mut inputs = Vec::with_capacity(self.entries.len());
        let mut any_animating = false;
        for toast in &self.entries {
            let label_text = format!("{}: {}", toast.kind.label(), toast.title);
            let has_detail = toast.detail.is_some();
            let detail_text = toast.detail.clone().unwrap_or_default();
            let icon_text = toast.kind.icon().glyph();
            let close_text = ChromeIcon::Close.glyph();

            let label = rs.text.measure(&label_text, label_size_px, font_manager);
            let detail = if has_detail {
                rs.text.measure(&detail_text, detail_size_px, font_manager)
            } else {
                ToastTextMetrics {
                    width: 0.0,
                    height: 0.0,
                    ascent: 0.0,
                }
            };
            let icon = rs.text.measure(&icon_text, icon_size_px, font_manager);
            // Rasterized at the same size as the kind icon, for visual
            // consistency between the leading icon and the trailing "x".
            let close_size_px = icon_size_px;
            let close = rs.text.measure(&close_text, close_size_px, font_manager);

            let anim = toast.anim(now);
            if anim.opacity < 1.0 || anim.slide_x.abs() > f32::EPSILON || anim.scale < 1.0 {
                any_animating = true;
            }

            inputs.push(ToastLayoutInput {
                id: toast.id,
                kind: toast.kind,
                label_text,
                label_size_px,
                label,
                detail_text,
                detail_size_px,
                detail,
                has_detail,
                icon_size_px,
                icon,
                close_size_px,
                close,
                anim,
            });
        }
        (inputs, any_animating)
    }

    /// Hover/dismiss hit-test: allocate one interactive click-only egui
    /// region per toast, over its `hit_rect_logical` (logical points).
    /// Hovering a toast's pill pauses its expiry (`last_hovered`); clicking
    /// anywhere within a toast's pill dismisses it — the whole pill is the
    /// click target, the drawn "x" glyph is purely a visual affordance.
    ///
    /// The regions live in an interactive `egui::Area` above the terminal so
    /// egui consumes the click (see the call site's finding-#3 comment). At
    /// most one toast is dismissed per frame, and regions are hit-tested
    /// newest-first (newest toasts are drawn on top) so that during the brief
    /// entry slide — when an arriving toast can transiently overlap its
    /// neighbour — a click in the overlap dismisses only the topmost one
    /// (review finding #4). Returns the ids to remove. Split out of
    /// [`Self::show`] to keep it under the line-count limit.
    fn hit_test(
        &mut self,
        ctx: &egui::Context,
        outputs: &[ToastLayoutOutput],
        now: Instant,
    ) -> Vec<u64> {
        // Allocate an interactive click region per toast so egui consumes any
        // click that lands on a toast (keeping it off the terminal). egui's
        // own arbitration reports the click on whichever region it picked; we
        // additionally derive the hover target and the (single, topmost)
        // dismiss target from the pointer position via `topmost_hit`, so the
        // overlap-precedence rule has one tested source of truth.
        let mut any_clicked = false;
        let pointer = egui::Area::new(egui::Id::new("toast_interaction"))
            .order(egui::Order::Foreground)
            .interactable(true)
            .fixed_pos(egui::Pos2::ZERO)
            .show(ctx, |ui| {
                for out in outputs {
                    let [min_x, min_y, max_x, max_y] = out.hit_rect_logical;
                    let rect = egui::Rect::from_min_max(
                        egui::pos2(min_x, min_y),
                        egui::pos2(max_x, max_y),
                    );
                    if ui.allocate_rect(rect, egui::Sense::click()).clicked() {
                        any_clicked = true;
                    }
                }
                ui.input(|i| i.pointer.hover_pos())
            })
            .inner;

        let Some(pos) = pointer else {
            return Vec::new();
        };
        let hovered = topmost_hit(outputs, pos);
        if let Some(id) = hovered
            && let Some(toast) = self.entries.iter_mut().find(|t| t.id == id)
        {
            toast.last_hovered = Some(now);
        }
        if any_clicked {
            hovered.into_iter().collect()
        } else {
            Vec::new()
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
                let gl = painter.gl();
                let vp = info.viewport_in_pixels();
                let mut rs = render_state_cb
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !rs.pill.initialized()
                    && let Err(e) = rs.pill.init(gl)
                {
                    tracing::error!("toast pill GL init failed: {e}");
                    return;
                }
                if !rs.text.initialized()
                    && let Err(e) = rs.text.init(gl)
                {
                    tracing::error!("toast text GL init failed: {e}");
                    return;
                }
                rs.pill.draw(gl, &pills, vp.width_px, vp.height_px);
                rs.text
                    .upload_and_draw(gl, &text_instances, vp.width_px, vp.height_px);
            })),
        });
    }
}

/// Whether logical-point `pos` falls within `rect` (`[min_x, min_y, max_x,
/// max_y]`, logical points — see [`ToastLayoutOutput::hit_rect_logical`]).
fn rect_contains(rect: [f32; 4], pos: egui::Pos2) -> bool {
    pos.x >= rect[0] && pos.x <= rect[2] && pos.y >= rect[1] && pos.y <= rect[3]
}

/// The id of the topmost toast whose hit rect contains `pos`, or `None`.
///
/// "Topmost" = newest = last in `outputs` (drawn on top), so this iterates in
/// reverse. Pure and GL-/egui-free so the overlap-precedence rule (review
/// finding #4) is unit-testable; the interactive [`ToastStack::hit_test`]
/// egui path relies on the same newest-first ordering when it allocates its
/// click regions.
fn topmost_hit(outputs: &[ToastLayoutOutput], pos: egui::Pos2) -> Option<u64> {
    outputs
        .iter()
        .rev()
        .find(|out| rect_contains(out.hit_rect_logical, pos))
        .map(|out| out.id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn stack_starts_empty() {
        let s = ToastStack::default();
        assert!(s.entries.is_empty());
    }

    #[test]
    fn prefers_dark_text_picks_by_luminance() {
        // Near-white fill → dark text; near-black fill → light text.
        assert!(ToastKind::prefers_dark_text(egui::Color32::from_gray(240)));
        assert!(!ToastKind::prefers_dark_text(egui::Color32::from_gray(20)));
    }

    #[test]
    fn background_derives_from_visuals_semantic_colors() {
        // The bubble fill must be a blend of the kind's palette semantic color
        // and the panel background — not a hard-coded hue. Use distinct
        // semantic colors so each kind yields a distinct fill.
        let mut v = egui::Visuals::dark();
        v.error_fg_color = egui::Color32::from_rgb(200, 0, 0);
        v.warn_fg_color = egui::Color32::from_rgb(0, 200, 0);
        v.hyperlink_color = egui::Color32::from_rgb(0, 0, 200);
        v.panel_fill = egui::Color32::from_gray(0);

        let err = ToastKind::Error.background(&v);
        let warn = ToastKind::Warning.background(&v);
        let info = ToastKind::Info.background(&v);

        // Each is a 65% accent / 35% black blend → dominant channel ~130.
        assert!(
            err.r() > err.g() && err.r() > err.b(),
            "error bubble is red-dominant"
        );
        assert!(
            warn.g() > warn.r() && warn.g() > warn.b(),
            "warning bubble is green-dominant"
        );
        assert!(
            info.b() > info.r() && info.b() > info.g(),
            "info bubble is blue-dominant"
        );
        assert_eq!(err.a(), 240, "bubble opacity is 240");
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
            id: 0,
            created,
            last_hovered: None,
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
            id: 0,
            created,
            // Hover event coincides with `later` — within the 200 ms
            // keep-alive window.
            last_hovered: Some(later),
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
            id: 0,
            created,
            // Hover was 5 s ago — well past HOVER_HOLD + ANIM_OUT.
            last_hovered: Some(stale_hover),
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
            id: 0,
            created,
            last_hovered,
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
    //  layout_toasts — pure geometry
    // -----------------------------------------------------------------

    const SETTLED_ANIM: ToastAnim = ToastAnim {
        opacity: 1.0,
        slide_x: 0.0,
        scale: 1.0,
    };

    const ENTRY_ANIM: ToastAnim = ToastAnim {
        opacity: 0.0,
        slide_x: SLIDE_IN_PTS,
        scale: SCALE_IN,
    };

    /// Build a [`ToastLayoutInput`] from simple numbers, so layout tests
    /// never need real shaping/rasterisation.
    fn test_input(
        id: u64,
        label_width: f32,
        has_detail: bool,
        anim: ToastAnim,
    ) -> ToastLayoutInput {
        ToastLayoutInput {
            id,
            kind: ToastKind::Info,
            label_text: "Info: hello".to_owned(),
            label_size_px: 16.0,
            label: ToastTextMetrics {
                width: label_width,
                height: 20.0,
                ascent: 15.0,
            },
            detail_text: if has_detail {
                "detail line".to_owned()
            } else {
                String::new()
            },
            detail_size_px: 13.0,
            detail: ToastTextMetrics {
                width: 80.0,
                height: 16.0,
                ascent: 12.0,
            },
            has_detail,
            icon_size_px: 16.0,
            icon: ToastTextMetrics {
                width: 16.0,
                height: 16.0,
                ascent: 12.0,
            },
            close_size_px: 16.0,
            close: ToastTextMetrics {
                width: 16.0,
                height: 16.0,
                ascent: 12.0,
            },
            anim,
        }
    }

    fn test_window_rect() -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(800.0, 600.0))
    }

    #[test]
    fn top_right_stack_places_second_toast_below_first() {
        let inputs = [
            test_input(0, 100.0, false, SETTLED_ANIM),
            test_input(1, 100.0, false, SETTLED_ANIM),
        ];
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(&inputs, ToastPosition::TopRightStack, geom, &visuals, 1.0);

        assert_eq!(out.len(), 2);
        let gap = TOAST_STACK_GAP_PTS; // ppp == 1.0
        assert!(
            (out[1].pill.y - out[0].pill.y - (out[0].pill.height + gap)).abs() < 0.01,
            "second toast must sit `height + gap` below the first: {} vs {}",
            out[1].pill.y,
            out[0].pill.y + out[0].pill.height + gap
        );

        // Both right edges align near the window's right edge.
        let right0 = out[0].pill.x + out[0].pill.width;
        let right1 = out[1].pill.x + out[1].pill.width;
        assert!((right0 - right1).abs() < 0.01);
        let expected_right = test_window_rect().max.x - STACK_RIGHT_INSET_PTS;
        assert!((right0 - expected_right).abs() < 0.01);
    }

    #[test]
    fn wider_label_produces_wider_pill_up_to_clamp() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();

        let narrow = layout_toasts(
            &[test_input(0, 50.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let wide = layout_toasts(
            &[test_input(0, 200.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!(wide[0].pill.width > narrow[0].pill.width);

        // An extremely long label must clamp to the max pill width.
        let huge = layout_toasts(
            &[test_input(0, 10_000.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!((huge[0].pill.width - MAX_PILL_WIDTH_PTS).abs() < 0.01);
    }

    #[test]
    fn detail_line_adds_height() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();

        let without = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let with_detail = layout_toasts(
            &[test_input(0, 100.0, true, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!(with_detail[0].pill.height > without[0].pill.height);
        // Detail run must actually be emitted; every toast also gets a
        // trailing dismiss-glyph run.
        assert_eq!(
            with_detail[0].runs.len(),
            4,
            "icon + label + detail + close"
        );
        assert_eq!(without[0].runs.len(), 3, "icon + label + close only");
    }

    #[test]
    fn entry_animation_starts_transparent_and_displaced() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();

        let entry = layout_toasts(
            &[test_input(0, 100.0, false, ENTRY_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        let settled = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );

        assert!(entry[0].pill.opacity.abs() < f32::EPSILON);
        assert!(
            entry[0].pill.x > settled[0].pill.x,
            "entry pill (x={}) must be displaced right of the settled pill (x={})",
            entry[0].pill.x,
            settled[0].pill.x
        );
        for run in &entry[0].runs {
            assert!(
                run.color[3].abs() < f32::EPSILON,
                "entry text must be transparent"
            );
        }
        assert!(settled[0].pill.x.is_finite());
        assert!((settled[0].pill.opacity - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn settled_animation_is_unslid_and_opaque() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        );
        assert!((out[0].pill.opacity - 1.0).abs() < f32::EPSILON);
        // The icon/label/detail runs are fully opaque when settled; the
        // trailing dismiss-glyph run is intentionally dimmed to read as a
        // secondary affordance (see `build_text_runs`), so it is checked
        // separately: still visible, but strictly less opaque.
        let (close_run, primary_runs) = out[0].runs.split_last().expect("at least one run");
        for run in primary_runs {
            assert!((run.color[3] - 1.0).abs() < f32::EPSILON);
        }
        assert!(
            close_run.color[3] > 0.0,
            "close glyph must still be visible"
        );
        assert!(
            close_run.color[3] < 1.0,
            "close glyph is dimmed relative to primary text"
        );
    }

    #[test]
    fn window_centered_centers_pill_in_window_rect() {
        let window_rect = test_window_rect();
        let geom = ToastGeometry {
            window_rect,
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::WindowCentered,
            geom,
            &visuals,
            1.0,
        );
        let pill = &out[0].pill;
        let center_x = pill.x + pill.width / 2.0;
        let center_y = pill.y + pill.height / 2.0;
        assert!((center_x - window_rect.center().x).abs() < 0.01);
        assert!((center_y - window_rect.center().y).abs() < 0.01);
    }

    #[test]
    fn pane_centered_uses_active_pane_rect() {
        let window_rect = test_window_rect();
        let pane_rect =
            egui::Rect::from_min_size(egui::pos2(400.0, 300.0), egui::vec2(400.0, 300.0));
        let geom = ToastGeometry {
            window_rect,
            active_pane_rect: Some(pane_rect),
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::PaneCentered,
            geom,
            &visuals,
            1.0,
        );
        let pill = &out[0].pill;
        let center_x = pill.x + pill.width / 2.0;
        let center_y = pill.y + pill.height / 2.0;
        assert!((center_x - pane_rect.center().x).abs() < 0.01);
        assert!((center_y - pane_rect.center().y).abs() < 0.01);
        // Sanity: the pane center differs from the window center in this test.
        assert!((pane_rect.center().x - window_rect.center().x).abs() > 1.0);
    }

    #[test]
    fn pane_centered_falls_back_to_window_rect_when_no_active_pane() {
        let window_rect = test_window_rect();
        let geom = ToastGeometry {
            window_rect,
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::PaneCentered,
            geom,
            &visuals,
            1.0,
        );
        let pill = &out[0].pill;
        let center_x = pill.x + pill.width / 2.0;
        let center_y = pill.y + pill.height / 2.0;
        assert!((center_x - window_rect.center().x).abs() < 0.01);
        assert!((center_y - window_rect.center().y).abs() < 0.01);
    }

    #[test]
    fn hit_rect_logical_divides_out_pixels_per_point() {
        let ppp = 2.0;
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            ppp,
        );
        let pill = &out[0].pill;
        let hit = out[0].hit_rect_logical;
        assert!((hit[0] - pill.x / ppp).abs() < 0.01);
        assert!((hit[1] - pill.y / ppp).abs() < 0.01);
        assert!((hit[2] - (pill.x + pill.width) / ppp).abs() < 0.01);
        assert!((hit[3] - (pill.y + pill.height) / ppp).abs() < 0.01);
    }

    #[test]
    fn output_id_matches_input_id() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let inputs = [
            test_input(42, 100.0, false, SETTLED_ANIM),
            test_input(7, 100.0, false, SETTLED_ANIM),
        ];
        let out = layout_toasts(&inputs, ToastPosition::TopRightStack, geom, &visuals, 1.0);
        assert_eq!(out[0].id, 42);
        assert_eq!(out[1].id, 7);
    }

    #[test]
    fn close_glyph_run_sits_inside_the_pill_near_the_right_padding() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        let ppp = 1.0;
        let out = layout_toasts(
            &[test_input(0, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            ppp,
        );
        let pill = &out[0].pill;
        // icon + label + close, in that order (no detail on this toast).
        assert_eq!(out[0].runs.len(), 3, "icon + label + close");
        let close_run = &out[0].runs[2];
        assert_eq!(
            close_run.text,
            ChromeIcon::Close.glyph(),
            "final run is the dismiss glyph"
        );

        let pill_left = pill.x;
        let pill_right = pill.x + pill.width;
        let pill_top = pill.y;
        let pill_bottom = pill.y + pill.height;

        // The close glyph sits right of the label origin, within the pill's
        // horizontal extent, and inset from the pill's right edge (the
        // padding reserved for the dismiss button).
        let label_run = &out[0].runs[1];
        assert!(
            close_run.origin_x > label_run.origin_x,
            "close glyph is right of the label"
        );
        assert!(close_run.origin_x > pill_left, "close glyph inside pill");
        assert!(
            close_run.origin_x < pill_right,
            "close glyph inset from the pill's right edge"
        );
        assert!(close_run.baseline_y >= pill_top);
        assert!(close_run.baseline_y <= pill_bottom);
    }

    #[test]
    fn empty_input_produces_empty_output() {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        assert!(layout_toasts(&[], ToastPosition::TopRightStack, geom, &visuals, 1.0).is_empty());
    }

    // -----------------------------------------------------------------
    //  ToastStack::hit_test — click-anywhere-on-the-pill dismissal
    // -----------------------------------------------------------------

    /// Build layout outputs for a single settled error toast (helper for the
    /// `topmost_hit` geometry tests). The interactive `hit_test` egui path
    /// itself needs a live egui context and is verified manually; these tests
    /// cover the pure hit-precedence geometry it relies on.
    fn single_toast_outputs(id: u64) -> Vec<ToastLayoutOutput> {
        let geom = ToastGeometry {
            window_rect: test_window_rect(),
            active_pane_rect: None,
        };
        let visuals = egui::Visuals::dark();
        layout_toasts(
            &[test_input(id, 100.0, false, SETTLED_ANIM)],
            ToastPosition::TopRightStack,
            geom,
            &visuals,
            1.0,
        )
    }

    #[test]
    fn topmost_hit_matches_a_click_anywhere_in_the_pill() {
        let outputs = single_toast_outputs(7);
        let pill = &outputs[0].pill;
        // The pill's center — nowhere near the drawn "x" glyph at the right
        // edge — must still match (the whole pill is the click target).
        let center = egui::pos2(pill.x + pill.width / 2.0, pill.y + pill.height / 2.0);
        assert_eq!(topmost_hit(&outputs, center), Some(7));
    }

    #[test]
    fn topmost_hit_misses_a_click_outside_the_pill() {
        let outputs = single_toast_outputs(7);
        let pill = &outputs[0].pill;
        let outside = egui::pos2(pill.x - 50.0, pill.y - 50.0);
        assert_eq!(topmost_hit(&outputs, outside), None);
    }

    #[test]
    fn topmost_hit_prefers_the_newest_toast_on_overlap() {
        // Two toasts whose hit rects overlap; the newest (last in `outputs`,
        // drawn on top) must win the overlap (review finding #4).
        let older = ToastLayoutOutput {
            id: 1,
            pill: ToastQuad {
                x: 0.0,
                y: 0.0,
                width: 10.0,
                height: 10.0,
                corner_radius: 0.0,
                color_top: [0.0; 4],
                color_bottom: [0.0; 4],
                glow: [0.0; 4],
                accent: [0.0; 4],
                opacity: 1.0,
            },
            runs: Vec::new(),
            hit_rect_logical: [0.0, 0.0, 100.0, 100.0],
        };
        let mut newer = older.clone();
        newer.id = 2;
        newer.hit_rect_logical = [50.0, 50.0, 150.0, 150.0];
        let outputs = vec![older, newer];
        // A point inside both rects must resolve to the newer toast (id 2).
        assert_eq!(topmost_hit(&outputs, egui::pos2(75.0, 75.0)), Some(2));
        // A point only in the older rect resolves to it.
        assert_eq!(topmost_hit(&outputs, egui::pos2(10.0, 10.0)), Some(1));
    }

    // -----------------------------------------------------------------
    //  Pure color helpers
    // -----------------------------------------------------------------

    #[test]
    fn color32_to_rgba_is_straight_normalized() {
        let c = egui::Color32::from_rgba_unmultiplied(255, 128, 0, 64);
        let rgba = color32_to_rgba(c);
        assert!((rgba[0] - 1.0).abs() < f32::EPSILON);
        assert!((rgba[1] - 128.0 / 255.0).abs() < f32::EPSILON);
        assert!(rgba[2].abs() < f32::EPSILON);
        assert!((rgba[3] - 64.0 / 255.0).abs() < f32::EPSILON);
    }

    #[test]
    fn lighten_shifts_rgb_and_clamps_leaving_alpha_untouched() {
        let base = [0.5, 0.5, 0.5, 0.9];
        let lighter = lighten(base, 0.4);
        assert!((lighter[0] - 0.9).abs() < f32::EPSILON);
        assert!((lighter[3] - 0.9).abs() < f32::EPSILON);

        let clamped = lighten(base, 10.0);
        assert!((clamped[0] - 1.0).abs() < f32::EPSILON);

        let darker = lighten(base, -10.0);
        assert!(darker[0].abs() < f32::EPSILON);
    }
}
