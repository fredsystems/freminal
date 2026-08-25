// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The Group A (#122.4) published-frame-state type: the home for
//! [`PerWindowState`](super::window::PerWindowState) fields that exist
//! purely to cross the frame boundary. See [`PublishedFrameState`]'s doc
//! for the full publish/read discipline.

use super::chrome_damage::ChromeSignals;
use super::panes::PaneId;
use super::window::ResizeOverlayState;

/// The exact geometry and input-suppression state the out-of-frame
/// immediate PTY mouse-report path (Task 124.3a,
/// `terminal::pty_mouse_report`) needs for one pane.
///
/// Published once per frame from [`super::terminal::FreminalTerminalWidget::show`]
/// via `PaneRenderCache::pointer_report_inputs`: computed once, in
/// `show()`, lifted verbatim by `app_impl` immediately after `show()`
/// returns — never recomputed here or anywhere else, so this can never
/// silently drift from what `show()` actually drew/suppressed this frame.
///
/// This is also the sole source of a pane's published terminal-rect
/// origin: `terminal_rect.min` IS that origin, so
/// [`PublishedFrameState::pane_terminal_origin`] derives from this type's
/// `terminal_rect` rather than a second, parallel map (the original
/// subtask 122.15 design, removed by Task 124.3a's review once this type
/// existed and made the second copy redundant).
///
/// Unknown/unpublished (`PublishedFrameState::pane_pointer_report_inputs`
/// returns `None`) means "no immediate report can be sent for this pane" —
/// conservative on delivery. It never suppresses repaint scheduling, which
/// is a wholly separate axis (`App::pointer_motion_needs_repaint`).
// Mirrors `InputSuppressors`'s own allow (`terminal/widget.rs`) for the
// same reason: each bool field is a separate, independently-observed
// suppressor condition, and every combination is legal and meaningful —
// the correct use of bools per `freminal-state-representation`, not a
// state machine masquerading as bools.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PanePointerReportInputs {
    /// This pane's terminal content rect (post-gutter-inset), egui logical
    /// points, top-left origin — the exact rect the frame-time
    /// `Event::PointerMoved` handler tests `contains(pos)` against.
    pub(super) terminal_rect: egui::Rect,
    /// One character cell's logical size (width, height), egui points.
    pub(super) cell_size: egui::Vec2,
    /// `ctx.pixels_per_point()` as of this frame — needed to convert a
    /// logical-point offset into the physical-pixel coordinates
    /// `MouseEncoding::SgrPixels` (`?1016`) reports.
    pub(super) pixels_per_point: f32,
    /// Mirrors `InputSuppressors::modal_or_drag`.
    pub(super) modal_or_drag: bool,
    /// Mirrors `InputSuppressors::context_menu`.
    pub(super) context_menu: bool,
    /// Mirrors `InputSuppressors::search_overlay`.
    pub(super) search_overlay: bool,
    /// Mirrors `InputSuppressors::command_history`.
    pub(super) command_history: bool,
    /// Mirrors `InputSuppressors::scrollbar_drag`.
    pub(super) scrollbar_drag: bool,
    /// The scrollbar's exact hit rect (`terminal/widget.rs`'s
    /// `scrollbar_hit_rect` helper), egui logical points, `Some` only when
    /// the thumb actually rendered this frame (`ScrollbarOutcome::hit_rect`
    /// — see that field's doc for why a not-rendered scrollbar publishes
    /// `None` rather than a rect nothing can be hovering). Task 124.3b: the
    /// out-of-frame pointer-motion predicate compares this exact rect
    /// across the previous and current pointer position to decide whether
    /// motion crossed the scrollbar's hover boundary, replacing the old
    /// pane-wide `scroll_offset > 0` veto.
    pub(super) scrollbar_hit_rect: Option<egui::Rect>,
}

impl PanePointerReportInputs {
    /// Whether ANY suppressor is active — mirrors `InputSuppressors::any()`.
    pub(super) const fn any_suppressor(self) -> bool {
        self.modal_or_drag
            || self.context_menu
            || self.search_overlay
            || self.command_history
            || self.scrollbar_drag
    }
}

impl Default for PanePointerReportInputs {
    /// The "no frame has published yet" value: an empty rect and cell
    /// size, unity pixel scale, and no suppressors active. Never read
    /// directly as "safe to report" — callers must go through
    /// [`PublishedFrameState::pane_pointer_report_inputs`], which returns
    /// `None` until a real frame publishes, rather than this default.
    fn default() -> Self {
        Self {
            terminal_rect: egui::Rect::ZERO,
            cell_size: egui::Vec2::ZERO,
            pixels_per_point: 1.0,
            modal_or_drag: false,
            context_menu: false,
            search_overlay: false,
            command_history: false,
            scrollbar_drag: false,
            scrollbar_hit_rect: None,
        }
    }
}

/// State written during one [`App::update`](freminal_windowing::App::update)
/// call and read only from **outside** any frame.
///
/// This is the Group A (#122.4) home for what were seven separate
/// [`PerWindowState`](super::window::PerWindowState) fields — the cached
/// menu/tab-bar, split-border and toast-pill rects, the cached central
/// content rect and gutter inset, the staged §3.3 chrome signals, and the
/// resize-overlay HUD state. They had no collective name or enforced
/// invariant before this type; see
/// `Documents/PLAN_122_ORCHESTRATION_EXTRACTION.md`'s "122.1 audit result"
/// section for the line-by-line inventory this type was built from. It is a
/// **wrapper, not a redesign**: every field keeps its pre-existing type,
/// including `chrome_head_rects`'s `Option<Vec<egui::Rect>>` — that
/// `None`/`Some(vec![])` distinction is semantic (no frame has rendered
/// yet, vs. one rendered and produced no head rects) and must not be
/// conflated with its two plain-`Vec` siblings.
///
/// # Publish discipline
///
/// Each field is written **at most once per successfully-completing
/// `App::update`**, at a fixed point in that function, and from nowhere
/// else — all seven fields unconditionally once that point is reached.
///
/// Reads happen **exclusively** from two predicates on `FreminalGui`:
/// `is_chrome_interactive_at` and `pointer_motion_needs_repaint`, both in
/// `app_impl.rs`, which `freminal-windowing` calls from its `CursorMoved`
/// pointer fast path (`freminal-windowing/src/event_loop.rs`, the
/// `WindowEvent::CursorMoved` arm) — a control path fully decoupled from
/// `update()`, running between frames rather than inside one.
///
/// Anchors in this doc comment are deliberately **function and branch names,
/// not line numbers**. Task 122 rewrites large parts of `app_impl.rs` across
/// several subtasks, and every line number cited here had already drifted by
/// the time 122.4 landed. Names survive that; numbers do not.
///
/// A value published this way is therefore **one frame stale by
/// construction, even in the best case**: a read may observe the exact same
/// snapshot across arbitrarily many pointer events between two frames, since
/// nothing re-publishes between them.
///
/// # Early-return staleness
///
/// All four of `App::update`'s early-return paths — settings-window
/// dispatch, dead-window cleanup, the last-tab-closes-the-window case, and
/// the no-active-pane bail-out — leave **every** field holding whatever the
/// last fully-completing `update()` left.
///
/// The decisive detail is ordering: the two paths that actually hold a
/// `PerWindowState` (last-tab-close and no-active-pane) both `return`
/// *before* chrome construction begins, which contains the earliest write
/// in this type (`chrome_head_rects`). Every other write site is later
/// still. So on both paths the `PerWindowState` —
/// and this type inside it — is reinserted into `self.windows` completely
/// untouched, layering an early-return staleness on top of the ordinary
/// one-frame staleness above. The other two paths are vacuous: the
/// settings window has no `PerWindowState` at all, and the dead-window
/// cleanup path found none to reinsert.
///
/// # Write ordering (load-bearing — do not collapse)
///
/// The three rect-publishing writes must keep firing in this relative
/// order, because [`super::chrome_damage::point_in_chrome_rects`]'s callers
/// rely on each being either freshly written or correctly still-stale by
/// the time they read it within the same frame:
///
/// 1. `chrome_head_rects` — published early, inside the menu/tab-bar
///    construction.
/// 2. `chrome_border_rects` — published inside the `central_body` closure,
///    or cleared there when no drag sensors were built that frame.
/// 3. `chrome_toast_rects` — pre-cleared, then republished if the toast
///    stack actually rendered, **after** `central_body` returns.
///
/// A caller that read `chrome_toast_rects` mid-`central_body` (before step
/// 3) would observe a value the current frame has not yet had the chance to
/// overwrite — reintroducing exactly the staleness class this discipline
/// exists to bound. Collapsing the order (e.g. publishing toast rects
/// before the border rects are known) would have the same effect.
#[derive(Debug, Default)]
pub(super) struct PublishedFrameState {
    /// The `CentralPanel` content rect captured on the most recent frame.
    /// Read by the out-of-frame pointer-motion predicates (which have no
    /// live `available_rect` to compute it from) and by the toast-rendering
    /// block, which renders outside `central_body`.
    cached_central_rect: Option<egui::Rect>,
    /// Menu-bar + tab-bar rects, captured every frame. `None` until the
    /// first frame renders — see the type doc for why this stays
    /// `Option<Vec<_>>` rather than unifying with its two siblings below.
    chrome_head_rects: Option<Vec<egui::Rect>>,
    /// Split-border drag-sensor rects, rebuilt every frame; explicitly
    /// cleared on frames that build no sensors (single pane / zoomed /
    /// overlay open).
    chrome_border_rects: Vec<egui::Rect>,
    /// The most recently laid-out toast pill hit-rects, appended whenever
    /// the app-level toast-rendering block actually runs `ToastStack::show`.
    chrome_toast_rects: Vec<egui::Rect>,
    /// The individual #436 §3.3 signals computed during the most recent
    /// `update()`, staged here because the final chrome-damage decision
    /// also needs a post-toast-render sample only available after
    /// `central_body` returns.
    pending_chrome_signals: ChromeSignals,
    /// Active resize-overlay HUD state, or `None` when no resize is in
    /// progress / the overlay has timed out (issue #433).
    resize_overlay: Option<ResizeOverlayState>,
    /// Each live pane's [`PanePointerReportInputs`] as of the most recent
    /// frame — the geometry and suppressor snapshot the out-of-frame
    /// immediate PTY mouse-report path (Task 124.3a) reads, and also the
    /// sole source of [`Self::pane_terminal_origin`]'s answer (see that
    /// method's doc). Same rebuilt-every-frame, clear-before-republish
    /// discipline as `chrome_border_rects`: cleared once before the
    /// per-pane render loop begins, then exactly one entry republished per
    /// still-live pane, so a pane closed since the last frame never leaves
    /// a stale entry behind.
    pane_pointer_report_inputs: std::collections::HashMap<PaneId, PanePointerReportInputs>,
}

impl PublishedFrameState {
    /// A fresh instance, matching the values a newly created window's
    /// `PerWindowState` needs before its first frame: no cached rects, no
    /// head rects yet, empty rect lists, default (all-false) chrome
    /// signals, and no resize overlay.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// The `CentralPanel` content rect cached on the most recent frame,
    /// or `None` before the first one has rendered.
    pub(super) const fn cached_central_rect(&self) -> Option<egui::Rect> {
        self.cached_central_rect
    }

    /// Publish this frame's `CentralPanel` content rect.
    pub(super) const fn publish_cached_central_rect(&mut self, rect: egui::Rect) {
        self.cached_central_rect = Some(rect);
    }

    /// The menu-bar + tab-bar rects from the most recent frame, or
    /// `None` if no frame has rendered yet.
    pub(super) fn chrome_head_rects(&self) -> Option<&[egui::Rect]> {
        self.chrome_head_rects.as_deref()
    }

    /// Publish this frame's menu-bar + tab-bar rects.
    pub(super) fn publish_chrome_head_rects(&mut self, rects: Vec<egui::Rect>) {
        self.chrome_head_rects = Some(rects);
    }

    /// The split-border drag-sensor rects from the most recent frame.
    pub(super) fn chrome_border_rects(&self) -> &[egui::Rect] {
        &self.chrome_border_rects
    }

    /// Publish this frame's split-border drag-sensor rects.
    pub(super) fn publish_chrome_border_rects(&mut self, rects: Vec<egui::Rect>) {
        self.chrome_border_rects = rects;
    }

    /// Clear the split-border drag-sensor rects (no sensors were built this
    /// frame — single pane / zoomed / overlay open).
    pub(super) fn clear_chrome_border_rects(&mut self) {
        self.chrome_border_rects.clear();
    }

    /// The toast pill hit-rects from the most recent frame that ran
    /// `ToastStack::show`.
    pub(super) fn chrome_toast_rects(&self) -> &[egui::Rect] {
        &self.chrome_toast_rects
    }

    /// Publish this frame's toast pill hit-rects.
    pub(super) fn publish_chrome_toast_rects(&mut self, rects: Vec<egui::Rect>) {
        self.chrome_toast_rects = rects;
    }

    /// Pre-clear the toast pill hit-rects before deciding whether the toast
    /// stack actually renders this frame — see the type doc's write-ordering
    /// section for why this must happen unconditionally on every frame.
    pub(super) fn clear_chrome_toast_rects(&mut self) {
        self.chrome_toast_rects.clear();
    }

    /// The staged #436 §3.3 chrome signals from the most recent frame.
    pub(super) const fn pending_chrome_signals(&self) -> ChromeSignals {
        self.pending_chrome_signals
    }

    /// Publish this frame's staged #436 §3.3 chrome signals.
    pub(super) const fn publish_pending_chrome_signals(&mut self, signals: ChromeSignals) {
        self.pending_chrome_signals = signals;
    }

    /// The active resize-overlay HUD state, or `None` when no resize is in
    /// progress / the overlay has timed out.
    pub(super) const fn resize_overlay(&self) -> Option<ResizeOverlayState> {
        self.resize_overlay
    }

    /// Start (or refresh) the resize-overlay HUD for this frame.
    pub(super) const fn start_resize_overlay(&mut self, overlay: ResizeOverlayState) {
        self.resize_overlay = Some(overlay);
    }

    /// Clear the resize-overlay HUD (its linger window elapsed).
    pub(super) const fn clear_resize_overlay(&mut self) {
        self.resize_overlay = None;
    }

    /// The terminal-rect origin published for `pane_id` on the most recent
    /// frame that rendered it, or `None` if that pane has never published
    /// one (not yet rendered, or closed and never replaced this slot).
    ///
    /// Derived from [`Self::pane_pointer_report_inputs`]'s
    /// `terminal_rect.min` rather than a second, parallel map: subtask
    /// 122.15 originally built a dedicated `pane_terminal_origins` map for
    /// this, but Task 124.3a's own `PanePointerReportInputs` publishes the
    /// identical `terminal_rect` (whose `.min` corner IS this origin) every
    /// frame for the same pane, so the second map had no independent
    /// reader and only risked drifting from it. This getter now reads that
    /// single source of truth, converted from `egui::Pos2` via
    /// `geometry_interop::point_from_egui` — the crate's sanctioned
    /// egui-to-toolkit-neutral crossing point.
    ///
    /// TODO(121.17): no production code calls this getter yet, so it has
    /// no caller outside the round-trip tests below until subtask 121.17
    /// (124.3b) wires its cell-granular suppression check through it. It
    /// is real, finished, production-shaped API (not a throwaway), so the
    /// `allow` below is temporary rather than permanent; remove it when
    /// 121.17/124.3b adds its first production call site.
    #[allow(dead_code)]
    pub(super) fn pane_terminal_origin(
        &self,
        pane_id: PaneId,
    ) -> Option<freminal_common::geometry::Point> {
        self.pane_pointer_report_inputs
            .get(&pane_id)
            .map(|inputs| super::geometry_interop::point_from_egui(inputs.terminal_rect.min))
    }

    /// The [`PanePointerReportInputs`] published for `pane_id` on the most
    /// recent frame that rendered it, or `None` if that pane has never
    /// published one (not yet rendered, or closed and never replaced this
    /// slot) — the "no immediate report" conservative default (Task 124.3a).
    pub(super) fn pane_pointer_report_inputs(
        &self,
        pane_id: PaneId,
    ) -> Option<PanePointerReportInputs> {
        self.pane_pointer_report_inputs.get(&pane_id).copied()
    }

    /// Whether ANY live pane's most-recently-published `scrollbar_drag`
    /// suppressor is active (Task 124.3b).
    ///
    /// The out-of-frame pointer-motion repaint predicate cannot rely on
    /// only the currently-resolved pane's own flag: `handle_scrollbar`
    /// keeps tracking `primary_down()` and updating the offset regardless
    /// of which pane the pointer currently resolves to, so a drag started
    /// in one pane can continue while the pointer strays outside that
    /// pane's rect (or over no pane at all). Checking every published
    /// pane's flag, rather than one resolved pane's, is what makes the
    /// force unconditional the way an in-progress drag needs.
    pub(super) fn any_pane_scrollbar_dragging(&self) -> bool {
        self.pane_pointer_report_inputs
            .values()
            .any(|inputs| inputs.scrollbar_drag)
    }

    /// Publish `pane_id`'s [`PanePointerReportInputs`] for this frame, as
    /// computed by `FreminalTerminalWidget::show` and recorded into that
    /// pane's `PaneRenderCache`.
    pub(super) fn publish_pane_pointer_report_inputs(
        &mut self,
        pane_id: PaneId,
        inputs: PanePointerReportInputs,
    ) {
        self.pane_pointer_report_inputs.insert(pane_id, inputs);
    }

    /// Clear every published `PanePointerReportInputs`. Called once, before
    /// the per-pane render loop begins, so a pane closed since the last
    /// frame does not leave a stale entry behind.
    pub(super) fn clear_pane_pointer_report_inputs(&mut self) {
        self.pane_pointer_report_inputs.clear();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::{PanePointerReportInputs, PublishedFrameState};
    use crate::gui::chrome_damage::ChromeSignals;
    use crate::gui::panes::PaneIdGenerator;
    use crate::gui::window::ResizeOverlayState;

    fn rect(x: f32, y: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(10.0, 10.0))
    }

    fn overlay(cols: usize, rows: usize) -> ResizeOverlayState {
        ResizeOverlayState {
            size: (cols, rows),
            last_update: std::time::Instant::now(),
        }
    }

    /// Pin 1: a fresh instance matches what all three real construction
    /// sites initialise today.
    #[test]
    fn fresh_instance_has_documented_initial_values() {
        let state = PublishedFrameState::new();
        assert_eq!(state.cached_central_rect(), None);
        assert_eq!(state.chrome_head_rects(), None);
        assert!(state.chrome_border_rects().is_empty());
        assert!(state.chrome_toast_rects().is_empty());
        assert_eq!(state.pending_chrome_signals(), ChromeSignals::default());
        assert!(state.resize_overlay().is_none());
    }

    /// `Default` agrees with `new()` (both are the "before the first frame"
    /// state).
    #[test]
    fn default_matches_new() {
        let default_state = PublishedFrameState::default();
        let new_state = PublishedFrameState::new();
        assert_eq!(
            default_state.cached_central_rect(),
            new_state.cached_central_rect()
        );
        assert_eq!(
            default_state.chrome_head_rects(),
            new_state.chrome_head_rects()
        );
    }

    /// Pin 2: publishing then reading returns exactly what was published.
    #[test]
    fn publish_then_read_round_trips() {
        let mut state = PublishedFrameState::new();

        let central = rect(1.0, 2.0);
        state.publish_cached_central_rect(central);
        assert_eq!(state.cached_central_rect(), Some(central));

        let head = vec![rect(0.0, 0.0), rect(0.0, 20.0)];
        state.publish_chrome_head_rects(head.clone());
        assert_eq!(state.chrome_head_rects(), Some(head.as_slice()));

        let border = vec![rect(50.0, 0.0)];
        state.publish_chrome_border_rects(border.clone());
        assert_eq!(state.chrome_border_rects(), border.as_slice());

        let toast = vec![rect(100.0, 0.0)];
        state.publish_chrome_toast_rects(toast.clone());
        assert_eq!(state.chrome_toast_rects(), toast.as_slice());

        let signals = ChromeSignals {
            any_overlay_open: true,
            bell_active: true,
            ..ChromeSignals::default()
        };
        state.publish_pending_chrome_signals(signals);
        assert_eq!(state.pending_chrome_signals(), signals);

        let hud = overlay(80, 24);
        state.start_resize_overlay(hud);
        let read_back = state
            .resize_overlay()
            .expect("resize overlay was just published");
        assert_eq!(read_back.size, hud.size);
        assert_eq!(read_back.last_update, hud.last_update);
    }

    /// Pin 3: the invariant this type exists to carry. A frame that
    /// early-returns, or completes only partially, leaves every field it
    /// did not write holding the previous frame's value for the next
    /// out-of-frame read.
    #[test]
    fn frame_that_does_not_write_a_field_leaves_the_previous_value() {
        let mut state = PublishedFrameState::new();

        // "Frame 1" fully completes and publishes real values.
        let central = rect(5.0, 5.0);
        state.publish_cached_central_rect(central);
        state.publish_chrome_head_rects(vec![rect(0.0, 0.0)]);
        state.publish_chrome_border_rects(vec![rect(10.0, 10.0)]);
        state.publish_chrome_toast_rects(vec![rect(20.0, 20.0)]);
        let signals = ChromeSignals {
            active_pane_changed: true,
            ..ChromeSignals::default()
        };
        state.publish_pending_chrome_signals(signals);
        let hud = overlay(120, 40);
        state.start_resize_overlay(hud);

        // "Frame 2" is a synthetic partial write — it does not correspond
        // to any real `update()` frame today (every field is written
        // unconditionally once chrome construction begins), but an
        // early-return path (settings-window dispatch, dead-window
        // cleanup, no-active-pane) leaves every field untouched, and this
        // is the general form of that: a frame that writes SOME fields but
        // not others must still leave the rest holding the previous
        // frame's value. This is also what makes this test falsifiable —
        // asserting after a frame that wrote nothing would hold for any
        // struct with plain getters and could not fail.
        state.publish_cached_central_rect(rect(6.0, 6.0));
        state.publish_chrome_border_rects(vec![rect(11.0, 11.0)]);

        // Frame 2's writes landed...
        assert_eq!(state.cached_central_rect(), Some(rect(6.0, 6.0)));
        assert_eq!(state.chrome_border_rects(), [rect(11.0, 11.0)].as_slice());

        // ...and every field frame 2 did NOT write still holds frame 1's
        // value, including `chrome_head_rects` — the reason it is `Option`
        // rather than a plain `Vec` (see the type's doc comment) is exactly
        // so a field that has never been written is distinguishable from
        // one written and empty.
        assert_eq!(state.chrome_head_rects(), Some([rect(0.0, 0.0)].as_slice()));
        assert_eq!(state.chrome_toast_rects(), [rect(20.0, 20.0)].as_slice());
        assert_eq!(state.pending_chrome_signals(), signals);
        let read_back = state
            .resize_overlay()
            .expect("resize overlay survives a frame that does not touch it");
        assert_eq!(read_back.size, hud.size);
    }

    /// Pin 4a: `chrome_border_rects`'s real clear-then-write pattern
    /// (the publish/clear branch pair inside `central_body`) — a write after a
    /// clear leaves exactly
    /// the written rects, not the union of both.
    #[test]
    fn chrome_border_rects_clear_then_write() {
        let mut state = PublishedFrameState::new();
        state.publish_chrome_border_rects(vec![rect(0.0, 0.0)]);

        state.clear_chrome_border_rects();
        assert!(state.chrome_border_rects().is_empty());

        let fresh = vec![rect(30.0, 30.0)];
        state.publish_chrome_border_rects(fresh.clone());
        assert_eq!(state.chrome_border_rects(), fresh.as_slice());
    }

    /// Pin 4b: a clear with no subsequent write (the "no sensors built this
    /// frame" branch) leaves the field empty, NOT holding the previous
    /// frame's rects.
    #[test]
    fn chrome_border_rects_clear_with_no_write_stays_empty() {
        let mut state = PublishedFrameState::new();
        state.publish_chrome_border_rects(vec![rect(0.0, 0.0), rect(5.0, 5.0)]);

        state.clear_chrome_border_rects();

        assert!(state.chrome_border_rects().is_empty());
    }

    /// Pin 4c: `chrome_toast_rects`'s real pre-clear-then-write pattern
    /// (the pre-clear before the toast pass, then the publish after it) — same
    /// shape as the border rects, kept as
    /// a separate test since it is a structurally distinct field (its own
    /// dedicated slot, not shared with `chrome_border_rects`).
    #[test]
    fn chrome_toast_rects_pre_clear_then_write() {
        let mut state = PublishedFrameState::new();
        state.publish_chrome_toast_rects(vec![rect(0.0, 0.0)]);

        state.clear_chrome_toast_rects();
        assert!(state.chrome_toast_rects().is_empty());

        let fresh = vec![rect(40.0, 40.0)];
        state.publish_chrome_toast_rects(fresh.clone());
        assert_eq!(state.chrome_toast_rects(), fresh.as_slice());
    }

    /// Pin 4d: pre-clearing `chrome_toast_rects` with no subsequent write
    /// (the toast stack was empty this frame) leaves it empty rather than
    /// stale — this is the exact bug #122.4's doc references (a stale
    /// non-empty stack permanently forcing chrome-interactive over a
    /// now-vacated region).
    #[test]
    fn chrome_toast_rects_pre_clear_with_no_write_stays_empty() {
        let mut state = PublishedFrameState::new();
        state.publish_chrome_toast_rects(vec![rect(0.0, 0.0), rect(1.0, 1.0)]);

        state.clear_chrome_toast_rects();

        assert!(state.chrome_toast_rects().is_empty());
    }

    /// Pin 5: `chrome_head_rects`'s `None` vs `Some(vec![])` distinction is
    /// preserved — these are different states (no frame yet, vs. a frame
    /// that built no head rects) and must not be conflated.
    #[test]
    fn chrome_head_rects_none_and_some_empty_are_distinct() {
        let mut state = PublishedFrameState::new();
        assert_eq!(state.chrome_head_rects(), None);

        state.publish_chrome_head_rects(Vec::new());
        assert_eq!(state.chrome_head_rects(), Some([].as_slice()));
        assert_ne!(state.chrome_head_rects(), None);
    }

    /// The resize-overlay clear path (on linger timeout).
    #[test]
    fn resize_overlay_start_then_clear() {
        let mut state = PublishedFrameState::new();
        state.start_resize_overlay(overlay(80, 24));
        assert!(state.resize_overlay().is_some());

        state.clear_resize_overlay();
        assert!(state.resize_overlay().is_none());
    }

    /// A fresh instance has no published terminal-rect origin (no pane has
    /// rendered yet, so `pane_pointer_report_inputs` has nothing to derive
    /// from).
    #[test]
    fn fresh_instance_has_no_pane_terminal_origins() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane = id_gen.next_id();
        let state = PublishedFrameState::new();
        assert_eq!(state.pane_terminal_origin(pane), None);
    }

    // ── Task 124.3a: `PanePointerReportInputs` / its publish discipline ──

    /// No suppressor active — the common case in these tests.
    fn report_inputs_clean() -> PanePointerReportInputs {
        PanePointerReportInputs {
            terminal_rect: rect(1.0, 2.0),
            cell_size: egui::vec2(8.0, 16.0),
            pixels_per_point: 1.5,
            modal_or_drag: false,
            context_menu: false,
            search_overlay: false,
            command_history: false,
            scrollbar_drag: false,
            scrollbar_hit_rect: None,
        }
    }

    /// Same geometry as [`report_inputs_clean`], but with `modal_or_drag`
    /// active — used where a test specifically needs a suppressed pane.
    fn report_inputs_with_modal_or_drag() -> PanePointerReportInputs {
        PanePointerReportInputs {
            modal_or_drag: true,
            ..report_inputs_clean()
        }
    }

    #[test]
    fn any_suppressor_is_true_iff_any_field_is_true() {
        assert!(!PanePointerReportInputs::default().any_suppressor());
        assert!(report_inputs_with_modal_or_drag().any_suppressor());
        assert!(!report_inputs_clean().any_suppressor());

        assert!(
            PanePointerReportInputs {
                scrollbar_drag: true,
                ..report_inputs_clean()
            }
            .any_suppressor()
        );
    }

    // ── Task 124.3b: `scrollbar_hit_rect` ────────────────────────────────

    #[test]
    fn scrollbar_hit_rect_defaults_to_none() {
        assert_eq!(PanePointerReportInputs::default().scrollbar_hit_rect, None);
        assert_eq!(report_inputs_clean().scrollbar_hit_rect, None);
    }

    #[test]
    fn scrollbar_hit_rect_round_trips_when_published() {
        let hit_rect = rect(90.0, 0.0);
        let inputs = PanePointerReportInputs {
            scrollbar_hit_rect: Some(hit_rect),
            ..report_inputs_clean()
        };
        let mut id_gen = PaneIdGenerator::new(0);
        let pane = id_gen.next_id();
        let mut state = PublishedFrameState::new();
        state.publish_pane_pointer_report_inputs(pane, inputs);

        assert_eq!(
            state
                .pane_pointer_report_inputs(pane)
                .and_then(|i| i.scrollbar_hit_rect),
            Some(hit_rect)
        );
    }

    // ── `pane_terminal_origin`, derived from `pane_pointer_report_inputs`
    // (subtask 122.15, re-derived by Task 124.3a's review to remove the
    // former parallel `pane_terminal_origins` map) ──────────────────────

    /// Publishing a pane's `PanePointerReportInputs` makes
    /// `pane_terminal_origin` return exactly `terminal_rect.min` (converted
    /// to the toolkit-neutral `Point`), and does not affect another pane's
    /// (never-published) slot.
    #[test]
    fn pane_terminal_origin_is_derived_and_keyed_per_pane() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let pane_b = id_gen.next_id();
        let mut state = PublishedFrameState::new();

        let inputs_a = PanePointerReportInputs {
            terminal_rect: egui::Rect::from_min_size(
                egui::pos2(12.0, 34.0),
                egui::vec2(10.0, 10.0),
            ),
            ..report_inputs_clean()
        };
        state.publish_pane_pointer_report_inputs(pane_a, inputs_a);

        assert_eq!(
            state.pane_terminal_origin(pane_a),
            Some(freminal_common::geometry::point(12.0, 34.0))
        );
        assert_eq!(state.pane_terminal_origin(pane_b), None);
    }

    /// Pin: the exact per-frame lifecycle documented on
    /// `pane_pointer_report_inputs` — `clear_pane_pointer_report_inputs`
    /// (called once before the per-pane loop) followed by one
    /// `publish_pane_pointer_report_inputs` per still-live pane — must NOT
    /// leave a closed pane's stale origin behind, now that
    /// `pane_terminal_origin` derives from that same map.
    #[test]
    fn pane_terminal_origin_clear_then_republish_drops_closed_panes() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let pane_b = id_gen.next_id();
        let mut state = PublishedFrameState::new();

        // "Frame 1": both panes are live and publish an origin.
        let first_geometry_for_pane_a = PanePointerReportInputs {
            terminal_rect: egui::Rect::from_min_size(egui::pos2(1.0, 1.0), egui::vec2(10.0, 10.0)),
            ..report_inputs_clean()
        };
        let first_geometry_for_pane_b = PanePointerReportInputs {
            terminal_rect: egui::Rect::from_min_size(egui::pos2(2.0, 2.0), egui::vec2(10.0, 10.0)),
            ..report_inputs_clean()
        };
        state.publish_pane_pointer_report_inputs(pane_a, first_geometry_for_pane_a);
        state.publish_pane_pointer_report_inputs(pane_b, first_geometry_for_pane_b);
        assert!(state.pane_terminal_origin(pane_a).is_some());
        assert!(state.pane_terminal_origin(pane_b).is_some());

        // "Frame 2": pane_b was closed. The per-pane loop clears the whole
        // map first, then republishes only the panes still in that frame's
        // `pane_layout` — here, only pane_a.
        state.clear_pane_pointer_report_inputs();
        let updated_geometry_for_pane_a = PanePointerReportInputs {
            terminal_rect: egui::Rect::from_min_size(egui::pos2(9.0, 9.0), egui::vec2(10.0, 10.0)),
            ..report_inputs_clean()
        };
        state.publish_pane_pointer_report_inputs(pane_a, updated_geometry_for_pane_a);

        assert_eq!(
            state.pane_terminal_origin(pane_a),
            Some(freminal_common::geometry::point(9.0, 9.0))
        );
        assert_eq!(
            state.pane_terminal_origin(pane_b),
            None,
            "a closed pane's stale origin must not survive the clear"
        );
    }

    #[test]
    fn fresh_instance_has_no_pane_pointer_report_inputs() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane = id_gen.next_id();
        let state = PublishedFrameState::new();
        assert_eq!(state.pane_pointer_report_inputs(pane), None);
    }

    #[test]
    fn pane_pointer_report_inputs_publish_then_read_is_keyed_per_pane() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let pane_b = id_gen.next_id();
        let mut state = PublishedFrameState::new();

        let inputs_a = report_inputs_clean();
        state.publish_pane_pointer_report_inputs(pane_a, inputs_a);

        assert_eq!(state.pane_pointer_report_inputs(pane_a), Some(inputs_a));
        assert_eq!(state.pane_pointer_report_inputs(pane_b), None);
    }

    #[test]
    fn pane_pointer_report_inputs_clear_then_republish_drops_closed_panes() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let pane_b = id_gen.next_id();
        let mut state = PublishedFrameState::new();

        state.publish_pane_pointer_report_inputs(pane_a, report_inputs_clean());
        state.publish_pane_pointer_report_inputs(pane_b, report_inputs_clean());
        assert!(state.pane_pointer_report_inputs(pane_a).is_some());
        assert!(state.pane_pointer_report_inputs(pane_b).is_some());

        state.clear_pane_pointer_report_inputs();
        let new_inputs_a = report_inputs_with_modal_or_drag();
        state.publish_pane_pointer_report_inputs(pane_a, new_inputs_a);

        assert_eq!(state.pane_pointer_report_inputs(pane_a), Some(new_inputs_a));
        assert_eq!(
            state.pane_pointer_report_inputs(pane_b),
            None,
            "a closed pane's stale pointer-report inputs must not survive the clear"
        );
    }

    // ── Task 124.3b: `any_pane_scrollbar_dragging` ───────────────────────

    #[test]
    fn any_pane_scrollbar_dragging_false_when_nothing_published() {
        assert!(!PublishedFrameState::new().any_pane_scrollbar_dragging());
    }

    #[test]
    fn any_pane_scrollbar_dragging_false_when_no_pane_is_dragging() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let mut state = PublishedFrameState::new();
        state.publish_pane_pointer_report_inputs(pane_a, report_inputs_clean());
        assert!(!state.any_pane_scrollbar_dragging());
    }

    #[test]
    fn any_pane_scrollbar_dragging_true_when_one_pane_is_dragging() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let pane_b = id_gen.next_id();
        let mut state = PublishedFrameState::new();
        state.publish_pane_pointer_report_inputs(pane_a, report_inputs_clean());
        state.publish_pane_pointer_report_inputs(
            pane_b,
            PanePointerReportInputs {
                scrollbar_drag: true,
                ..report_inputs_clean()
            },
        );
        assert!(state.any_pane_scrollbar_dragging());
    }

    #[test]
    fn any_pane_scrollbar_dragging_forgets_a_closed_panes_drag_after_clear() {
        let mut id_gen = PaneIdGenerator::new(0);
        let pane_a = id_gen.next_id();
        let mut state = PublishedFrameState::new();
        state.publish_pane_pointer_report_inputs(
            pane_a,
            PanePointerReportInputs {
                scrollbar_drag: true,
                ..report_inputs_clean()
            },
        );
        assert!(state.any_pane_scrollbar_dragging());

        state.clear_pane_pointer_report_inputs();
        assert!(!state.any_pane_scrollbar_dragging());
    }
}
