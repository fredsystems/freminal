// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! The Group A (#122.4) published-frame-state type: the home for
//! [`PerWindowState`](super::window::PerWindowState) fields that exist
//! purely to cross the frame boundary. See [`PublishedFrameState`]'s doc
//! for the full publish/read discipline.

use super::chrome_damage::ChromeSignals;
use super::window::ResizeOverlayState;

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
/// `None`/`Some(vec![])` distinction is semantic (no `Full` frame has
/// rendered yet, vs. one rendered and produced no head rects) and must not
/// be conflated with its two plain-`Vec` siblings.
///
/// # Publish discipline
///
/// Each field is written **at most once per successfully-completing
/// `App::update`**, at a fixed point in that function, and from nowhere
/// else — `chrome_head_rects` only on a `ChromeMode::Full` frame, the other
/// six unconditionally once that point is reached.
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
/// *before* the `ChromeMode::Full` vs `Replay` branch opens, and that branch
/// contains the earliest write in this type (`chrome_head_rects`). Every
/// other write site is later still. So on both paths the `PerWindowState` —
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
/// 1. `chrome_head_rects` — `Full`-frame-only, published early, inside the
///    menu/tab-bar construction branch.
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
    /// The `CentralPanel` content rect captured on the most recent FULL
    /// frame. On a REPLAY frame `App::update` skips building the menu bar,
    /// tab bar, and `CentralPanel` (all cached chrome), so there is no
    /// fresh `available_rect` to read the terminal band's content rect
    /// from — this cached value is used to construct an equivalent `Ui`
    /// directly, in the same background layer chrome uses.
    cached_central_rect: Option<egui::Rect>,
    /// The command-block gutter's total inset in logical points, cached
    /// each frame so the out-of-frame predicates can hit-test the gutter
    /// strip without `pixels_per_point` (which is only known inside a
    /// frame).
    cached_gutter_inset_logical: f32,
    /// Menu-bar + tab-bar rects, captured on FULL frames only. `None` until
    /// the first FULL frame renders — see the type doc for why this stays
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
}

impl PublishedFrameState {
    /// A fresh instance, matching the values a newly created window's
    /// `PerWindowState` needs before its first frame: no cached rects, a
    /// zero inset, no head rects yet, empty rect lists, default (all-false)
    /// chrome signals, and no resize overlay.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// The `CentralPanel` content rect cached on the most recent FULL
    /// frame, or `None` before the first one has rendered.
    pub(super) const fn cached_central_rect(&self) -> Option<egui::Rect> {
        self.cached_central_rect
    }

    /// Publish this frame's `CentralPanel` content rect.
    pub(super) const fn publish_cached_central_rect(&mut self, rect: egui::Rect) {
        self.cached_central_rect = Some(rect);
    }

    /// The command-block gutter's total inset in logical points, as of the
    /// most recent frame (`0.0` before the first).
    pub(super) const fn cached_gutter_inset_logical(&self) -> f32 {
        self.cached_gutter_inset_logical
    }

    /// Publish this frame's gutter inset.
    pub(super) const fn publish_cached_gutter_inset_logical(&mut self, inset: f32) {
        self.cached_gutter_inset_logical = inset;
    }

    /// The menu-bar + tab-bar rects from the most recent FULL frame, or
    /// `None` if no FULL frame has rendered yet.
    pub(super) fn chrome_head_rects(&self) -> Option<&[egui::Rect]> {
        self.chrome_head_rects.as_deref()
    }

    /// Publish this FULL frame's menu-bar + tab-bar rects.
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
    /// section for why this must happen unconditionally on every FULL frame.
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::PublishedFrameState;
    use crate::gui::chrome_damage::ChromeSignals;
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
        assert!((state.cached_gutter_inset_logical() - 0.0).abs() < f32::EPSILON);
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

        state.publish_cached_gutter_inset_logical(12.5);
        assert!((state.cached_gutter_inset_logical() - 12.5).abs() < f32::EPSILON);

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
    /// early-returns, or completes only partially (a `ChromeMode::Replay`
    /// frame), leaves every field it did not write holding the previous
    /// frame's value for the next out-of-frame read.
    #[test]
    fn frame_that_does_not_write_a_field_leaves_the_previous_value() {
        let mut state = PublishedFrameState::new();

        // "Frame 1" fully completes and publishes real values.
        let central = rect(5.0, 5.0);
        state.publish_cached_central_rect(central);
        state.publish_cached_gutter_inset_logical(8.0);
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

        // "Frame 2" is a `ChromeMode::Replay` frame: it reaches the
        // `central_body` writes but NOT the `Full`-only branch that
        // publishes `chrome_head_rects`. This is the realistic partial-write
        // case, and it is what makes this test falsifiable — asserting after
        // a frame that wrote nothing would hold for any struct with plain
        // getters and could not fail.
        state.publish_cached_central_rect(rect(6.0, 6.0));
        state.publish_chrome_border_rects(vec![rect(11.0, 11.0)]);

        // Frame 2's writes landed...
        assert_eq!(state.cached_central_rect(), Some(rect(6.0, 6.0)));
        assert_eq!(state.chrome_border_rects(), [rect(11.0, 11.0)].as_slice());

        // ...and every field frame 2 did NOT write still holds frame 1's
        // value. `chrome_head_rects` in particular survives a Replay frame:
        // that is the documented reason it is the one field written
        // `Full`-only, and the reason it is `Option` rather than a plain
        // `Vec` (see the type's doc comment).
        assert_eq!(state.chrome_head_rects(), Some([rect(0.0, 0.0)].as_slice()));
        assert_eq!(state.chrome_toast_rects(), [rect(20.0, 20.0)].as_slice());
        assert!((state.cached_gutter_inset_logical() - 8.0).abs() < f32::EPSILON);
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
    /// preserved — these are different states (no `Full` frame yet, vs. a
    /// `Full` frame that built no head rects) and must not be conflated.
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
}
