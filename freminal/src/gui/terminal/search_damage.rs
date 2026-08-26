// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Cross-frame search-overlay damage state (Task 124.14d).
//!
//! [`SearchDamageState`] is the one place that remembers what the search
//! overlay actually painted last frame -- the screen rows the search-match
//! tint was baked into, and the floating search-bar popup's paint bounds --
//! so [`super::widget::build_bounded_damage`] can union the old and new
//! extents into this frame's damage instead of forcing the whole pane (or,
//! before this subtask, the whole window) `Full` while search is open.
//!
//! Owned as one field on [`super::widget::PaneRenderCache`], per
//! `freminal-extend-or-extract`'s guidance to give cross-frame state a
//! named home rather than adding three loose fields to `widget.rs`.

use crate::gui::renderer::PaneDamageRect;
use crate::gui::search::SearchOverlaySafety;

/// What the search overlay painted last frame, and what it paints this
/// frame, kept as one focused piece of cross-frame state (Task 124.14d).
///
/// Two independent extents are tracked:
///
/// - **Highlight rows** ([`Self::replace_highlight_rows`]): the screen rows
///   the search-match tint was actually baked into at the last full vertex
///   rebuild. Updated only from inside that rebuild body -- the same
///   capture-before-overwrite discipline `PaneRenderCache::previous_selection`
///   already uses -- so a frame that reuses the previous frame's vertices
///   unchanged cannot advance the baseline.
/// - **Popup damage** ([`Self::finish_overlay_frame`]): the floating
///   search-bar's paint bounds. Unlike the highlight rows, this is
///   recomputed and unioned EVERY frame the widget runs, not only on a full
///   rebuild, because the bar's own caret/hover/text content can change
///   independently of any terminal-content rebuild.
///
/// Safety follows the same old/current union rule as popup geometry. A
/// tooltip can paint outside the popup bounds; when its control stops being
/// hovered, the next frame must still be unbounded so the old tooltip pixels
/// are erased. The frame after that settles back to bounded. This is a
/// one-frame settling rule, not sticky state.
#[derive(Debug, Clone)]
pub(super) struct SearchDamageState {
    /// Screen rows the search-match tint was baked into at the last full
    /// vertex rebuild. Sorted, deduplicated.
    previous_highlight_rows: Vec<usize>,
    /// The search-bar's paint bounds as of the most recent
    /// [`Self::finish_overlay_frame`] call. `None` once the bar has settled
    /// closed for at least one frame.
    previous_popup_rect: Option<PaneDamageRect>,
    /// This frame's popup damage: the deduplicated union of
    /// [`Self::previous_popup_rect`] (before it is overwritten) and the
    /// rect passed to the most recent [`Self::finish_overlay_frame`] call.
    /// Never carries more than two entries.
    current_popup_rects: Vec<PaneDamageRect>,
    /// The previous frame's search-overlay safety classification. Combined
    /// with the current input by [`Self::finish_overlay_frame`] so the frame
    /// that removes an escaped tooltip remains unbounded once.
    previous_safety: SearchOverlaySafety,
    /// This frame's effective search-overlay safety classification: the
    /// conservative union of previous and current safety.
    safety: SearchOverlaySafety,
}

impl SearchDamageState {
    /// A fresh state describing "search has never been open": no rows, no
    /// popup, bounded.
    pub(super) const fn new() -> Self {
        Self {
            previous_highlight_rows: Vec::new(),
            previous_popup_rect: None,
            current_popup_rects: Vec::new(),
            previous_safety: SearchOverlaySafety::Bounded,
            safety: SearchOverlaySafety::Bounded,
        }
    }

    /// Replace the recorded highlight-row baseline with `current` (sorted
    /// and deduplicated here -- not trusted from the caller), returning the
    /// OLD baseline so the caller can union it into this frame's damage
    /// alongside the new one: erasing a highlight that moved or
    /// disappeared needs the old rows just as much as drawing a new one
    /// needs the current ones.
    ///
    /// Call only from inside a full-rebuild body (mirrors
    /// `PaneRenderCache::previous_selection`'s capture-before-overwrite
    /// discipline): a frame that reuses the previous frame's vertices
    /// unchanged must not advance this baseline.
    pub(super) fn replace_highlight_rows(&mut self, mut current: Vec<usize>) -> Vec<usize> {
        current.sort_unstable();
        current.dedup();
        std::mem::replace(&mut self.previous_highlight_rows, current)
    }

    /// Record this frame's search-bar popup damage and safety
    /// classification.
    ///
    /// `current_popup_rect` is `Some` exactly when the bar was drawn this
    /// frame and its paint rect converted to a valid [`PaneDamageRect`];
    /// `None` when the bar was not drawn this frame, OR its rect
    /// degenerated during conversion -- the caller is responsible for
    /// passing [`SearchOverlaySafety::TooltipMayEscape`] in the latter
    /// case, since a degenerate rect is an unbounded-safety case, not a
    /// silently-dropped one.
    ///
    /// [`Self::overlay_damage_rects`] becomes the deduplicated union of the
    /// previous call's rect and this one: opening (`None` -> `Some`)
    /// damages only the new rect, a steady frame (`Some(r)` -> `Some(r)`)
    /// dedupes to one, a resize (`Some(a)` -> `Some(b)`, `a != b`) damages
    /// both, and closing (`Some(r)` -> `None`) damages the old rect once
    /// before settling empty on the next call.
    pub(super) fn finish_overlay_frame(
        &mut self,
        current_popup_rect: Option<PaneDamageRect>,
        safety: SearchOverlaySafety,
    ) {
        self.safety = self.previous_safety.combine(safety);
        self.previous_safety = safety;
        self.current_popup_rects.clear();
        match (self.previous_popup_rect, current_popup_rect) {
            (None, None) => {}
            (Some(prev), None) => self.current_popup_rects.push(prev),
            (None, Some(cur)) => self.current_popup_rects.push(cur),
            (Some(prev), Some(cur)) => {
                self.current_popup_rects.push(prev);
                if cur != prev {
                    self.current_popup_rects.push(cur);
                }
            }
        }
        self.previous_popup_rect = current_popup_rect;
    }

    /// This frame's popup damage rects (Task 124.14d), for union into
    /// `frame_damage::PaneDamageInput::search_overlay_rects`. Search
    /// highlight rows take the separate [`Self::replace_highlight_rows`]
    /// path into `build_bounded_damage`.
    pub(super) fn overlay_damage_rects(&self) -> &[PaneDamageRect] {
        &self.current_popup_rects
    }

    /// This frame's effective search-overlay safety classification. Includes
    /// one settling frame after [`SearchOverlaySafety::TooltipMayEscape`] so
    /// pixels outside the popup rect are erased safely.
    pub(super) const fn safety(&self) -> SearchOverlaySafety {
        self.safety
    }
}

impl Default for SearchDamageState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// A distinct, easily-told-apart `PaneDamageRect` rect for each of the
    /// small integer ids used below.
    const fn rect(id: i32) -> PaneDamageRect {
        PaneDamageRect {
            x: id,
            y: id,
            width: 10,
            height: 10,
        }
    }

    // ── finish_overlay_frame: popup damage union ────────────────────────

    /// Opening the bar for the first time: only the current rect is
    /// damaged, since there was nothing to erase.
    #[test]
    fn opening_damages_only_the_current_rect() {
        let mut state = SearchDamageState::new();

        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);

        assert_eq!(state.overlay_damage_rects(), &[rect(1)]);
    }

    /// A steady-open frame (identical rect both times) must still report
    /// exactly one rect -- deduplicated, not doubled -- and it must still
    /// be present every frame, since the bar's caret/hover state can change
    /// even when its bounds have not.
    #[test]
    fn steady_open_dedupes_to_one_rect_and_stays_present() {
        let mut state = SearchDamageState::new();
        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);

        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);
        assert_eq!(state.overlay_damage_rects(), &[rect(1)]);

        // A third identical frame must still report the rect -- it is not
        // a one-shot "changed" edge, it is unconditionally recomputed every
        // frame the bar is open.
        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);
        assert_eq!(state.overlay_damage_rects(), &[rect(1)]);
    }

    /// A resize (the bar's bounds genuinely change, e.g. "No matches" ->
    /// a match count, or a regex error appearing) must damage BOTH the old
    /// rect (erasing the previous bounds) and the new one (drawing the
    /// current bounds).
    #[test]
    fn resize_damages_both_old_and_new_rects() {
        let mut state = SearchDamageState::new();
        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);

        state.finish_overlay_frame(Some(rect(2)), SearchOverlaySafety::Bounded);

        assert_eq!(state.overlay_damage_rects(), &[rect(1), rect(2)]);
    }

    /// Closing: the frame the bar stops being drawn must still damage the
    /// PREVIOUS rect once (erasing it), and the frame after that must
    /// settle to empty -- the popup is gone and there is nothing left to
    /// erase.
    #[test]
    fn closing_damages_the_previous_rect_once_then_settles_empty() {
        let mut state = SearchDamageState::new();
        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);

        // The first frame after the close-action frame: `show_search_bar`
        // painted the bar before returning `Close`, so this is the first
        // frame on which the bar is actually absent.
        state.finish_overlay_frame(None, SearchOverlaySafety::Bounded);
        assert_eq!(
            state.overlay_damage_rects(),
            &[rect(1)],
            "the frame the bar stops being drawn must still erase its last rect"
        );

        // The following (settled-closed) frame: nothing left to damage.
        state.finish_overlay_frame(None, SearchOverlaySafety::Bounded);
        assert_eq!(
            state.overlay_damage_rects(),
            &[] as &[PaneDamageRect],
            "a settled-closed frame must report no popup damage at all"
        );
    }

    // ── finish_overlay_frame: one-frame safety settling ─────────────────

    /// A tooltip-escaping frame must force one settling unbounded frame after
    /// hover ends, so the old tooltip pixels outside the popup rect are
    /// erased. The following frame returns to bounded.
    #[test]
    fn tooltip_safety_settles_for_one_frame_before_returning_to_bounded() {
        let mut state = SearchDamageState::new();

        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::TooltipMayEscape);
        assert_eq!(state.safety(), SearchOverlaySafety::TooltipMayEscape);

        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);
        assert_eq!(
            state.safety(),
            SearchOverlaySafety::TooltipMayEscape,
            "the frame that removes an escaped tooltip must remain unbounded once"
        );

        state.finish_overlay_frame(Some(rect(1)), SearchOverlaySafety::Bounded);
        assert_eq!(
            state.safety(),
            SearchOverlaySafety::Bounded,
            "tooltip safety must settle after exactly one unbounded cleanup frame"
        );
    }

    // ── replace_highlight_rows ───────────────────────────────────────────

    /// The first call on a fresh state has nothing to return (no previous
    /// baseline), and must store the given rows sorted and deduplicated
    /// regardless of the order/duplicates they arrived in.
    #[test]
    fn highlight_row_replacement_returns_old_and_stores_sorted_deduped_current() {
        let mut state = SearchDamageState::new();

        let old = state.replace_highlight_rows(vec![5, 3, 3, 1]);
        assert_eq!(
            old,
            Vec::<usize>::new(),
            "a fresh state has no previous baseline to return"
        );

        // The second call's returned "old" value is exactly what the first
        // call stored -- proving it was sorted and deduplicated, not stored
        // verbatim as `[5, 3, 3, 1]`.
        let old = state.replace_highlight_rows(vec![2]);
        assert_eq!(old, vec![1, 3, 5]);
    }
}
