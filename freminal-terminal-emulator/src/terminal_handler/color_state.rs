// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! [`ColorState`]: every runtime colour input of a [`TerminalHandler`](super::TerminalHandler)
//! (the theme, the OSC 4 palette overrides and the OSC 10/11/12/17/19
//! dynamic overrides) plus a cached [`ResolvedColors`] derived from them.

use std::sync::Arc;

use freminal_common::{
    colors::{
        ColorPalette, DynamicColorOverrides, DynamicColorSlot, ResolvedColors, default_palette_rgb,
    },
    themes::ThemePalette,
};

/// The colour inputs of one terminal and the resolved colours derived from them.
///
/// # Cache invalidation rules
///
/// [`resolved`](Self::resolved) hands out an `Arc<ResolvedColors>` that is
/// rebuilt only when the content colours actually change; the GUI compares
/// successive Arcs with `Arc::ptr_eq` to detect a change, so a spurious
/// rebuild would force a needless repaint. Therefore:
///
/// - A mutator invalidates the cache **only if the effective colour changes**.
///   Overrides are always stored or cleared, but setting a palette or dynamic
///   entry to the colour it already shows (including the theme's own value),
///   resetting an override that equals the theme default, resetting an entry
///   that is already unset, and `set_theme` with an identical theme (same
///   pointer or equal value) all leave the cached Arc in place.
/// - Mutations of [`DynamicColorSlot::Cursor`] **never** invalidate: the
///   cursor colour is not part of [`ResolvedColors`] and travels separately.
/// - `reset_palette_all` invalidates only if some palette entry's effective
///   colour changes; `reset_dynamic_all` only if some non-cursor slot's
///   effective colour changes. The cursor slot is still cleared by
///   `reset_dynamic_all`.
#[derive(Debug)]
pub(super) struct ColorState {
    /// Active colour theme for default palette lookups.
    theme: &'static ThemePalette,
    /// Mutable 256-colour palette with optional per-index overrides (OSC 4).
    palette: ColorPalette,
    /// Dynamic colour overrides (OSC 10/11/12/17/19).
    dynamic: DynamicColorOverrides,
    /// Cached resolution of the above; `None` means stale.
    resolved: Option<Arc<ResolvedColors>>,
}

impl ColorState {
    /// A state with `theme`, no palette overrides and no dynamic overrides.
    pub(super) fn new(theme: &'static ThemePalette) -> Self {
        Self {
            theme,
            palette: ColorPalette::default(),
            dynamic: DynamicColorOverrides::default(),
            resolved: None,
        }
    }

    /// The active theme.
    pub(super) const fn theme(&self) -> &'static ThemePalette {
        self.theme
    }

    /// Switch to `theme`. An identical theme does not invalidate the cache.
    ///
    /// "Identical" is pointer equality or, failing that, value equality: a
    /// `&'static` taken from a `const` theme does not have a stable address,
    /// so pointer equality alone would miss genuinely identical themes.
    pub(super) fn set_theme(&mut self, theme: &'static ThemePalette) {
        let unchanged = std::ptr::eq(self.theme, theme) || *self.theme == *theme;
        self.theme = theme;
        if !unchanged {
            self.resolved = None;
        }
    }

    /// The OSC 4 palette overrides.
    pub(super) const fn palette(&self) -> &ColorPalette {
        &self.palette
    }

    /// The effective RGB value of palette entry `idx` (override or default).
    pub(super) fn palette_rgb(&self, idx: u8) -> (u8, u8, u8) {
        self.palette.rgb(idx, self.theme)
    }

    /// Override palette entry `idx` (OSC 4).
    ///
    /// The override is always stored (it must survive a later theme change),
    /// but the cache is invalidated only if the *effective* colour changes.
    pub(super) fn set_palette(&mut self, idx: u8, (r, g, b): (u8, u8, u8)) {
        let effective_before = self.palette.rgb(idx, self.theme);
        self.palette.set(idx, r, g, b);
        if effective_before != (r, g, b) {
            self.resolved = None;
        }
    }

    /// Clear the override of palette entry `idx` (OSC 104 with an index).
    ///
    /// Invalidates only if the effective colour changes.
    pub(super) fn reset_palette(&mut self, idx: u8) {
        let effective_before = self.palette.rgb(idx, self.theme);
        self.palette.reset(idx);
        if self.palette.rgb(idx, self.theme) != effective_before {
            self.resolved = None;
        }
    }

    /// Clear every palette override (OSC 104 without an index, RIS, DECSTR).
    pub(super) fn reset_palette_all(&mut self) {
        let effective_change = (0..=u8::MAX)
            .any(|idx| self.palette.rgb(idx, self.theme) != default_palette_rgb(idx, self.theme));
        self.palette.reset_all();
        if effective_change {
            self.resolved = None;
        }
    }

    /// The override of `slot`, or `None` if it follows the theme.
    pub(super) const fn dynamic(&self, slot: DynamicColorSlot) -> Option<(u8, u8, u8)> {
        self.dynamic.get(slot)
    }

    /// The colour `slot` currently shows: its override, else the theme's value.
    pub(super) const fn effective_dynamic(&self, slot: DynamicColorSlot) -> (u8, u8, u8) {
        match self.dynamic.get(slot) {
            Some(rgb) => rgb,
            None => self.theme_dynamic(slot),
        }
    }

    /// The theme's own value for `slot`, ignoring any override.
    const fn theme_dynamic(&self, slot: DynamicColorSlot) -> (u8, u8, u8) {
        match slot {
            DynamicColorSlot::Foreground => self.theme.foreground,
            DynamicColorSlot::Background => self.theme.background,
            DynamicColorSlot::Cursor => self.theme.cursor,
            DynamicColorSlot::SelectionBackground => self.theme.selection_bg,
            DynamicColorSlot::SelectionForeground => self.theme.selection_fg,
        }
    }

    /// Override `slot` (OSC 10/11/12/17/19).
    ///
    /// The override is always stored; the cache is invalidated only if the
    /// *effective* colour of a non-cursor slot changes.
    pub(super) fn set_dynamic(&mut self, slot: DynamicColorSlot, rgb: (u8, u8, u8)) {
        let effective_before = self.effective_dynamic(slot);
        self.dynamic.set(slot, rgb);
        if effective_before != rgb {
            self.invalidate_for(slot);
        }
    }

    /// Clear the override of `slot` (OSC 110/111/112/117/119).
    ///
    /// Invalidates only if the effective colour changes.
    pub(super) fn reset_dynamic(&mut self, slot: DynamicColorSlot) {
        let effective_before = self.effective_dynamic(slot);
        self.dynamic.reset(slot);
        if self.effective_dynamic(slot) != effective_before {
            self.invalidate_for(slot);
        }
    }

    /// Clear every dynamic override, the cursor colour included.
    ///
    /// Invalidates only if a non-cursor slot's effective colour changes.
    pub(super) fn reset_dynamic_all(&mut self) {
        let content_changes = DynamicColorSlot::ALL.into_iter().any(|slot| {
            slot != DynamicColorSlot::Cursor
                && self.effective_dynamic(slot) != self.theme_dynamic(slot)
        });
        self.dynamic.reset_all();
        if content_changes {
            self.resolved = None;
        }
    }

    /// The fully resolved content colours, rebuilt only if stale.
    pub(super) fn resolved(&mut self) -> Arc<ResolvedColors> {
        if let Some(cached) = &self.resolved {
            return Arc::clone(cached);
        }
        let fresh = Arc::new(ResolvedColors::resolve(
            self.theme,
            &self.palette,
            &self.dynamic,
        ));
        self.resolved = Some(Arc::clone(&fresh));
        fresh
    }

    /// Mark the cache stale after a change to `slot`, unless the slot is the
    /// cursor colour (which is not part of [`ResolvedColors`]).
    fn invalidate_for(&mut self, slot: DynamicColorSlot) {
        if slot != DynamicColorSlot::Cursor {
            self.resolved = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use freminal_common::themes::{CATPPUCCIN_MOCHA, DRACULA};

    fn state() -> ColorState {
        ColorState::new(&CATPPUCCIN_MOCHA)
    }

    /// Run `mutate`, then assert whether it produced a new Arc.
    fn changed_by(state: &mut ColorState, mutate: impl FnOnce(&mut ColorState)) -> bool {
        let before = state.resolved();
        mutate(state);
        let after = state.resolved();
        !Arc::ptr_eq(&before, &after)
    }

    #[test]
    fn repeated_resolved_shares_the_arc() {
        let mut s = state();
        let a = s.resolved();
        let b = s.resolved();
        assert!(Arc::ptr_eq(&a, &b));
    }

    #[test]
    fn set_palette_new_value_builds_new_arc() {
        let mut s = state();
        let before = s.resolved();
        s.set_palette(1, (1, 2, 3));
        let after = s.resolved();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.palette[1], (1, 2, 3));
    }

    #[test]
    fn set_palette_same_value_keeps_arc() {
        let mut s = state();
        s.set_palette(1, (1, 2, 3));
        assert!(!changed_by(&mut s, |s| s.set_palette(1, (1, 2, 3))));
    }

    #[test]
    fn reset_palette_of_set_entry_builds_new_arc() {
        let mut s = state();
        s.set_palette(1, (1, 2, 3));
        let before = s.resolved();
        s.reset_palette(1);
        let after = s.resolved();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.palette[1], CATPPUCCIN_MOCHA.ansi[1]);
    }

    #[test]
    fn reset_palette_of_unset_entry_keeps_arc() {
        let mut s = state();
        assert!(!changed_by(&mut s, |s| s.reset_palette(1)));
    }

    #[test]
    fn reset_palette_all_with_override_builds_new_arc() {
        let mut s = state();
        s.set_palette(200, (9, 9, 9));
        let before = s.resolved();
        s.reset_palette_all();
        let after = s.resolved();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_ne!(after.palette[200], (9, 9, 9));
    }

    #[test]
    fn reset_palette_all_with_nothing_set_keeps_arc() {
        let mut s = state();
        assert!(!changed_by(&mut s, ColorState::reset_palette_all));
    }

    #[test]
    fn set_dynamic_content_slots_build_new_arc() {
        for slot in [
            DynamicColorSlot::Foreground,
            DynamicColorSlot::Background,
            DynamicColorSlot::SelectionBackground,
            DynamicColorSlot::SelectionForeground,
        ] {
            let mut s = state();
            let before = s.resolved();
            s.set_dynamic(slot, (7, 8, 9));
            let after = s.resolved();
            assert!(!Arc::ptr_eq(&before, &after), "{slot:?}");
            let got = match slot {
                DynamicColorSlot::Foreground => after.foreground,
                DynamicColorSlot::Background => after.background,
                DynamicColorSlot::SelectionBackground => after.selection_background,
                DynamicColorSlot::SelectionForeground => after.selection_foreground,
                DynamicColorSlot::Cursor => unreachable!("cursor is not in the list"),
            };
            assert_eq!(got, (7, 8, 9), "{slot:?}");
        }
    }

    #[test]
    fn set_dynamic_same_value_keeps_arc() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Foreground, (7, 8, 9));
        assert!(!changed_by(&mut s, |s| {
            s.set_dynamic(DynamicColorSlot::Foreground, (7, 8, 9));
        }));
    }

    #[test]
    fn reset_dynamic_of_set_slot_builds_new_arc() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Background, (7, 8, 9));
        let before = s.resolved();
        s.reset_dynamic(DynamicColorSlot::Background);
        let after = s.resolved();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.background, CATPPUCCIN_MOCHA.background);
    }

    #[test]
    fn reset_dynamic_of_unset_slot_keeps_arc() {
        let mut s = state();
        assert!(!changed_by(&mut s, |s| {
            s.reset_dynamic(DynamicColorSlot::Foreground);
        }));
    }

    #[test]
    fn reset_dynamic_all_with_foreground_set_builds_new_arc() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Foreground, (7, 8, 9));
        let before = s.resolved();
        s.reset_dynamic_all();
        let after = s.resolved();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.foreground, CATPPUCCIN_MOCHA.foreground);
    }

    #[test]
    fn set_dynamic_to_exact_theme_value_from_unset_keeps_arc() {
        let mut s = state();
        assert!(!changed_by(&mut s, |s| {
            s.set_dynamic(DynamicColorSlot::Background, CATPPUCCIN_MOCHA.background);
        }));
        // The override is still stored.
        assert_eq!(
            s.dynamic(DynamicColorSlot::Background),
            Some(CATPPUCCIN_MOCHA.background)
        );
    }

    #[test]
    fn reset_dynamic_of_override_equal_to_theme_keeps_arc() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Foreground, CATPPUCCIN_MOCHA.foreground);
        assert!(!changed_by(&mut s, |s| {
            s.reset_dynamic(DynamicColorSlot::Foreground);
        }));
        assert_eq!(s.dynamic(DynamicColorSlot::Foreground), None);
    }

    #[test]
    fn set_dynamic_real_change_from_theme_override_builds_new_arc() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Foreground, CATPPUCCIN_MOCHA.foreground);
        assert!(changed_by(&mut s, |s| {
            s.set_dynamic(DynamicColorSlot::Foreground, (1, 2, 3));
        }));
    }

    #[test]
    fn reset_dynamic_all_with_only_theme_equal_overrides_keeps_arc() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Foreground, CATPPUCCIN_MOCHA.foreground);
        s.set_dynamic(DynamicColorSlot::Background, CATPPUCCIN_MOCHA.background);
        assert!(!changed_by(&mut s, ColorState::reset_dynamic_all));
        assert_eq!(s.dynamic(DynamicColorSlot::Foreground), None);
        assert_eq!(s.dynamic(DynamicColorSlot::Background), None);
    }

    #[test]
    fn reset_palette_all_with_only_default_equal_overrides_keeps_arc() {
        let mut s = state();
        s.set_palette(1, CATPPUCCIN_MOCHA.ansi[1]);
        s.set_palette(200, default_palette_rgb(200, &CATPPUCCIN_MOCHA));
        assert!(!changed_by(&mut s, ColorState::reset_palette_all));
        assert_eq!(*s.palette(), ColorPalette::default());
    }

    #[test]
    fn reset_dynamic_all_with_nothing_set_keeps_arc() {
        let mut s = state();
        assert!(!changed_by(&mut s, ColorState::reset_dynamic_all));
    }

    #[test]
    fn cursor_slot_mutations_never_invalidate() {
        let mut s = state();
        assert!(!changed_by(&mut s, |s| {
            s.set_dynamic(DynamicColorSlot::Cursor, (1, 2, 3));
        }));
        assert_eq!(s.dynamic(DynamicColorSlot::Cursor), Some((1, 2, 3)));
        assert!(!changed_by(&mut s, |s| {
            s.reset_dynamic(DynamicColorSlot::Cursor);
        }));
        assert_eq!(s.dynamic(DynamicColorSlot::Cursor), None);
    }

    #[test]
    fn reset_dynamic_all_with_only_cursor_set_keeps_arc_but_clears_cursor() {
        let mut s = state();
        s.set_dynamic(DynamicColorSlot::Cursor, (1, 2, 3));
        assert!(!changed_by(&mut s, ColorState::reset_dynamic_all));
        assert_eq!(s.dynamic(DynamicColorSlot::Cursor), None);
    }

    #[test]
    fn set_theme_different_builds_new_arc() {
        let mut s = state();
        let before = s.resolved();
        s.set_theme(&DRACULA);
        let after = s.resolved();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(after.foreground, DRACULA.foreground);
        assert!(std::ptr::eq(s.theme(), &DRACULA));
    }

    #[test]
    fn set_theme_same_keeps_arc() {
        let mut s = state();
        assert!(!changed_by(&mut s, |s| s.set_theme(&CATPPUCCIN_MOCHA)));
    }

    #[test]
    fn effective_dynamic_prefers_override_then_theme() {
        let mut s = state();
        assert_eq!(
            s.effective_dynamic(DynamicColorSlot::Cursor),
            CATPPUCCIN_MOCHA.cursor
        );
        assert_eq!(
            s.effective_dynamic(DynamicColorSlot::SelectionBackground),
            CATPPUCCIN_MOCHA.selection_bg
        );
        s.set_dynamic(DynamicColorSlot::Cursor, (1, 2, 3));
        assert_eq!(s.effective_dynamic(DynamicColorSlot::Cursor), (1, 2, 3));
    }

    #[test]
    fn palette_accessors_reflect_overrides() {
        let mut s = state();
        assert_eq!(s.palette_rgb(5), CATPPUCCIN_MOCHA.ansi[5]);
        s.set_palette(5, (10, 20, 30));
        assert_eq!(s.palette_rgb(5), (10, 20, 30));
        assert_eq!(s.palette().rgb(5, s.theme()), (10, 20, 30));
    }
}
