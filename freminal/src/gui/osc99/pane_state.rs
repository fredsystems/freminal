//! Where an OSC 99 request came from, and the state of that pane at the time:
//! what the `o=` occasion check consumes.

use freminal_windowing::WindowId;

use crate::gui::frame_drain::WindowFocus;
use crate::gui::panes::PaneId;

/// The pane an OSC 99 request came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99Origin {
    /// The OS window that owns the pane.
    pub window_id: WindowId,
    /// The pane that emitted the sequence.
    pub pane_id: PaneId,
}

/// Whether a pane has keyboard focus, in the sense the `o=unfocused` key
/// needs: the OS window is focused, the pane's tab is active, and the pane is
/// that tab's active pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKeyboardFocus {
    /// The pane would receive keyboard input right now.
    Focused,
    /// The pane would not receive keyboard input right now.
    Unfocused,
}

/// Whether a pane is on screen, in the sense the `o=invisible` key needs: the
/// OS window is focused and not minimised, the pane's tab is active, and no
/// other pane in that tab is zoomed over it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneVisibility {
    /// The pane is drawn on screen.
    Visible,
    /// The pane is not drawn on screen.
    Hidden,
}

/// The state of an OSC 99 request's originating pane at the time it was
/// drained, as consumed by the `o=` occasion check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99PaneState {
    /// Whether the pane has keyboard focus.
    pub focus: PaneKeyboardFocus,
    /// Whether the pane is on screen.
    pub visibility: PaneVisibility,
}

/// Whether the OS window is minimised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowMinimized {
    /// The window is minimised.
    Minimized,
    /// The window is not minimised.
    Restored,
}

impl WindowMinimized {
    /// Build from the raw `bool` read off the egui viewport info.
    pub const fn from_bool(minimized: bool) -> Self {
        if minimized {
            Self::Minimized
        } else {
            Self::Restored
        }
    }
}

/// Whether a pane's tab is the window's active tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabActivity {
    /// The tab is the window's active tab.
    Active,
    /// The tab is a background tab.
    Background,
}

/// Whether a pane is its tab's active pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneActivity {
    /// The pane is its tab's active pane.
    Active,
    /// The pane is not its tab's active pane.
    Inactive,
}

/// Whether another pane's zoom hides a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoomOcclusion {
    /// The pane's tab has no zoomed pane, or the zoomed pane is this one.
    NotOccluded,
    /// The pane's tab has a zoomed pane that is not this pane.
    Occluded,
}

impl Osc99PaneState {
    /// Derive a pane's state from the window, tab and pane inputs.
    ///
    /// - Focused iff the window is focused, the tab is active and the pane is
    ///   the tab's active pane.
    /// - Visible iff the window is focused and not minimised, the tab is
    ///   active, and no other pane is zoomed.
    #[must_use]
    pub const fn compute(
        window: WindowFocus,
        minimized: WindowMinimized,
        tab: TabActivity,
        pane: PaneActivity,
        zoom: ZoomOcclusion,
    ) -> Self {
        let window_focused = matches!(window, WindowFocus::Focused);
        let tab_active = matches!(tab, TabActivity::Active);

        let focus = if window_focused && tab_active && matches!(pane, PaneActivity::Active) {
            PaneKeyboardFocus::Focused
        } else {
            PaneKeyboardFocus::Unfocused
        };
        let visibility = if window_focused
            && matches!(minimized, WindowMinimized::Restored)
            && tab_active
            && matches!(zoom, ZoomOcclusion::NotOccluded)
        {
            PaneVisibility::Visible
        } else {
            PaneVisibility::Hidden
        };

        Self { focus, visibility }
    }
}

/// Where a pane's OSC 99 requests came from and what state it was in, built
/// once per pane per frame by the drain and handed to
/// `rendering::handle_window_manipulation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Osc99PaneContext {
    /// The pane the requests originate from.
    pub origin: Osc99Origin,
    /// The pane's state this frame.
    pub pane_state: Osc99PaneState,
}

#[cfg(test)]
mod tests {
    use super::{
        Osc99PaneState, PaneActivity, PaneKeyboardFocus, PaneVisibility, TabActivity,
        WindowMinimized, ZoomOcclusion,
    };
    use crate::gui::frame_drain::WindowFocus;

    const FOCUSED: WindowFocus = WindowFocus::Focused;
    const UNFOCUSED: WindowFocus = WindowFocus::Unfocused;
    const RESTORED: WindowMinimized = WindowMinimized::Restored;
    const MINIMIZED: WindowMinimized = WindowMinimized::Minimized;

    fn state(
        window: WindowFocus,
        minimized: WindowMinimized,
        tab: TabActivity,
        pane: PaneActivity,
        zoom: ZoomOcclusion,
    ) -> (PaneKeyboardFocus, PaneVisibility) {
        let s = Osc99PaneState::compute(window, minimized, tab, pane, zoom);
        (s.focus, s.visibility)
    }

    #[test]
    fn active_pane_of_active_tab_in_focused_window_is_focused_and_visible() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Focused, PaneVisibility::Visible)
        );
    }

    #[test]
    fn unfocused_window_makes_every_pane_unfocused_and_hidden() {
        assert_eq!(
            state(
                UNFOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn minimized_window_hides_the_pane_but_does_not_change_focus() {
        // Focus is "would receive keyboard input"; minimisation is a
        // visibility concept only, so a focused-but-minimised window (a
        // transient state some platforms report) keeps `Focused`.
        assert_eq!(
            state(
                FOCUSED,
                MINIMIZED,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Focused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn background_tab_is_unfocused_and_hidden_even_for_its_active_pane() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Background,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn inactive_pane_in_active_tab_is_unfocused_but_visible() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Inactive,
                ZoomOcclusion::NotOccluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Visible)
        );
    }

    #[test]
    fn pane_occluded_by_another_panes_zoom_is_hidden() {
        assert_eq!(
            state(
                FOCUSED,
                RESTORED,
                TabActivity::Active,
                PaneActivity::Inactive,
                ZoomOcclusion::Occluded
            ),
            (PaneKeyboardFocus::Unfocused, PaneVisibility::Hidden)
        );
    }

    #[test]
    fn exhaustive_inputs_match_the_stated_rules() {
        for window in [FOCUSED, UNFOCUSED] {
            for minimized in [RESTORED, MINIMIZED] {
                for tab in [TabActivity::Active, TabActivity::Background] {
                    for pane in [PaneActivity::Active, PaneActivity::Inactive] {
                        for zoom in [ZoomOcclusion::NotOccluded, ZoomOcclusion::Occluded] {
                            let s = Osc99PaneState::compute(window, minimized, tab, pane, zoom);
                            let expect_focused = window == FOCUSED
                                && tab == TabActivity::Active
                                && pane == PaneActivity::Active;
                            let expect_visible = window == FOCUSED
                                && minimized == RESTORED
                                && tab == TabActivity::Active
                                && zoom == ZoomOcclusion::NotOccluded;
                            assert_eq!(s.focus == PaneKeyboardFocus::Focused, expect_focused);
                            assert_eq!(s.visibility == PaneVisibility::Visible, expect_visible);
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn window_minimized_from_bool() {
        assert_eq!(WindowMinimized::from_bool(true), MINIMIZED);
        assert_eq!(WindowMinimized::from_bool(false), RESTORED);
    }
}
