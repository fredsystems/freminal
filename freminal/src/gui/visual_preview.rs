// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Live-preview state for the Settings window (issue #452 phase A).
//!
//! [`VisualPreview`] is the single source of truth for "what should the
//! running app look like right now, before the user has clicked Apply".
//! Everything that can be previewed while Settings is open -- the active
//! theme, the background opacity, the chrome style profile -- lives as a
//! field here.
//!
//! Reverting a live preview is **not** a separate concept: closing the
//! Settings window without applying simply previews the committed config
//! again (`VisualPreview::from_config(&committed_config, os_dark_mode)`).
//! There is deliberately no `Revert*` variant anywhere in this file or in
//! [`super::settings::SettingsAction`] -- treating revert as "preview the
//! old state" is what makes a forgotten revert structurally impossible.
//! Before this module existed, `PreviewProfile` had no matching revert at
//! all: cancelling a chrome-profile change left the picked profile applied
//! permanently, because the profile mutated `FreminalGui::gui_theme`
//! directly with nothing to put it back. That bug class cannot recur here,
//! because there is no per-option revert to forget.
//!
//! Phase C adds the font family/size/line-height triplet ([`FontPreview`]),
//! and with it the one genuine exception to "every preview applies
//! immediately": these three fields drive `FontManager::rebuild` (family,
//! line height) or `FontManager::set_font_size` (size), and applying either
//! on every intermediate slider/keystroke value would be wasteful (a
//! shaping-cache clear at minimum) or visibly janky (a full font reparse) —
//! see [`DebouncedPreview`] and [`FONT_PREVIEW_DEBOUNCE`]. Revert still
//! bypasses this debounce entirely and applies immediately: closing
//! Settings must not leave a previewed font lingering for the debounce
//! window, nor let a stale stash apply itself after the window is gone.
//! Because [`VisualPreview`] alone cannot distinguish "the draft changed"
//! from "the modal closed without applying" (both look like a new snapshot
//! to preview), [`SettingsAction::Preview`](super::settings::SettingsAction::Preview)
//! carries a [`PreviewTrigger`] alongside it purely to make that one
//! distinction — it is not a per-option revert and does not weaken the
//! whole-state-snapshot design above.
//!
//! Phase D adds the remaining expensive visual options: the background
//! image path and the shader path (both debounced through their own
//! [`DebouncedPreview`] instance, reusing phase C's generic holder
//! unchanged), and the command-block gutter position and tab bar position
//! (cheap combo boxes, applied immediately like the phase A/B fields). A
//! shader path is the one field whose preview can *fail* (a GLSL compile
//! error) — see [`ShaderErrorRoute`] for how a preview-time failure is kept
//! off the terminal windows' toast stack without weakening the toast
//! behaviour a committed (Apply) failure still gets.
use freminal_common::config::{
    BackgroundImageMode, Config, CursorShapeConfig, GutterPosition, TabBarPosition,
};
use freminal_common::gui_theme::StyleProfile;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The cursor-related subset of [`VisualPreview`]: shape, blink, and trail
/// (+ duration).
///
/// Grouped into its own type -- rather than four flat fields on
/// [`VisualPreview`] -- purely to keep that struct's own bool count under
/// clippy's `struct_excessive_bools` threshold (`blink` and `trail` are two
/// of `VisualPreview`'s four TOML-config-toggle bools; `ligatures` and
/// `hide_menu_bar` are the other two). The grouping doubles as a genuine
/// cohesive unit -- these four fields are exactly `CursorConfig` -- unlike
/// `FreminalTerminalWidget::WidgetDisplayToggles`, which groups otherwise
/// unrelated toggles for the same reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CursorPreview {
    /// Cursor shape (`CursorConfig::shape`).
    pub(super) shape: CursorShapeConfig,
    /// Cursor blink (`CursorConfig::blink`).
    pub(super) blink: bool,
    /// Cursor trail animation toggle (`CursorConfig::trail`).
    pub(super) trail: bool,
    /// Cursor trail animation duration in milliseconds
    /// (`CursorConfig::trail_duration_ms`).
    pub(super) trail_duration_ms: u32,
}

/// The font-rebuild-cost subset of [`VisualPreview`]: family, size, and line
/// height (issue #452 phase C).
///
/// All three change the cell size and route through the debounce holder
/// (see the module doc) rather than applying immediately like the phase A/B
/// fields. Grouped into its own type both for that shared debounce handling
/// and because it maps directly onto `FontConfig`'s three layout-affecting
/// fields (`ligatures` is phase B and stays a flat bool on [`VisualPreview`]
/// since it never needs debouncing).
#[derive(Debug, Clone)]
pub(super) struct FontPreview {
    /// Font family override (`FontConfig::family`). `None` means the
    /// bundled `CaskaydiaCove` default.
    pub(super) family: Option<String>,
    /// Font size in points (`FontConfig::size`).
    pub(super) size: f32,
    /// Line-height multiplier (`FontConfig::line_height`).
    pub(super) line_height: f32,
}

impl PartialEq for FontPreview {
    /// `size` and `line_height` are `f32`; compare with an epsilon for the
    /// same reason [`VisualPreview`]'s manual `PartialEq` does.
    fn eq(&self, other: &Self) -> bool {
        self.family == other.family
            && (self.size - other.size).abs() < f32::EPSILON
            && (self.line_height - other.line_height).abs() < f32::EPSILON
    }
}

impl FontPreview {
    /// Resolve the font-rebuild-cost subset of `config`.
    fn from_config(config: &Config) -> Self {
        Self {
            family: config.font.family.clone(),
            size: config.font.size,
            line_height: config.font.line_height,
        }
    }
}

/// The new logical font-family value reported by [`VisualPreviewDiff::font_family`]
/// when it changed.
///
/// A named enum rather than `Option<Option<String>>`: `font.family` is
/// already `Option<String>` (`None` meaning "bundled default"), and wrapping
/// that in another `Option` to mean "did this change" is exactly the
/// double-`Option` shape `clippy::option_option` exists to flag. This names
/// the same two logical outcomes instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum FontFamilyChange {
    /// Changed to a specific font family override.
    Custom(String),
    /// Changed back to (or newly set to) the bundled default (no override).
    Default,
}

impl FontFamilyChange {
    /// Build the appropriate variant from a `FontConfig::family`-shaped
    /// value.
    fn from_option(family: Option<&str>) -> Self {
        family.map_or(Self::Default, |f| Self::Custom(f.to_owned()))
    }
}

/// The new logical background-image path value reported by
/// [`VisualPreviewDiff::background_image_path`] when it changed (issue #452
/// phase D).
///
/// Same `Option<Option<PathBuf>>`-avoidance rationale as [`FontFamilyChange`]:
/// `UiConfig::background_image` is already `Option<PathBuf>` (`None` meaning
/// "no image"), so a second `Option` wrapping it to mean "did this change"
/// would be exactly the shape `clippy::option_option` flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum BackgroundImagePathChange {
    /// Changed to a specific background image path.
    Custom(PathBuf),
    /// Changed to (or newly set to) no background image.
    Cleared,
}

impl BackgroundImagePathChange {
    /// Build the appropriate variant from a `UiConfig::background_image`-shaped
    /// value.
    fn from_option(path: Option<&std::path::Path>) -> Self {
        path.map_or(Self::Cleared, |p| Self::Custom(p.to_path_buf()))
    }
}

/// The new logical shader path value reported by
/// [`VisualPreviewDiff::shader_path`] when it changed (issue #452 phase D).
///
/// Same `Option<Option<PathBuf>>`-avoidance rationale as [`FontFamilyChange`]
/// and [`BackgroundImagePathChange`], for `ShaderConfig::path`. Kept as its
/// own type rather than reusing [`BackgroundImagePathChange`] because the
/// two are different concepts (a GPU texture load versus a GLSL compile)
/// that only happen to share a shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ShaderPathChange {
    /// Changed to a specific shader path.
    Custom(PathBuf),
    /// Changed to (or newly set to) no shader (passthrough).
    Cleared,
}

impl ShaderPathChange {
    /// Build the appropriate variant from a `ShaderConfig::path`-shaped
    /// value.
    fn from_option(path: Option<&std::path::Path>) -> Self {
        path.map_or(Self::Cleared, |p| Self::Custom(p.to_path_buf()))
    }
}

/// The subset of [`Config`] that can be reflected live in the running app
/// while the Settings window is open, before the user applies.
///
/// This type is deliberately a **whole-state snapshot**, not a per-option
/// preview/revert event. Adding a previewable option in a later phase means
/// adding a field here -- nothing else -- so that preview and revert can
/// never drift apart.
///
/// Phase A (issue #452) covered the theme, background opacity, and chrome
/// style profile. Phase B adds the remaining "cheap" visual options that
/// need no font rebuild, no shader compile, no filesystem I/O and no PTY
/// resize: cursor shape/blink/trail(+duration) (see [`CursorPreview`]), font
/// ligatures, background image opacity/mode, and hiding the menu bar. Phase
/// C adds font family/size/line-height (see [`FontPreview`]), the one
/// subset that is debounced rather than applied immediately.
#[derive(Debug, Clone)]
pub struct VisualPreview {
    /// The resolved active theme slug (`ThemeConfig::active_slug`), already
    /// resolved against the OS dark/light preference for `ThemeMode::Auto`.
    pub(super) theme_slug: String,

    /// Background opacity (`UiConfig::background_opacity`).
    pub(super) background_opacity: f32,

    /// Chrome style profile (Modern/Retro).
    pub(super) profile: StyleProfile,

    /// Cursor shape/blink/trail(+duration). See [`CursorPreview`].
    pub(super) cursor: CursorPreview,

    /// OpenType ligature shaping toggle (`FontConfig::ligatures`).
    pub(super) ligatures: bool,

    /// Background image opacity (`UiConfig::background_image_opacity`).
    pub(super) background_image_opacity: f32,

    /// Background image fit mode (`UiConfig::background_image_mode`).
    pub(super) background_image_mode: BackgroundImageMode,

    /// Whether the menu bar is hidden (`UiConfig::hide_menu_bar`).
    pub(super) hide_menu_bar: bool,

    /// Font family/size/line-height. See [`FontPreview`]. Debounced (issue
    /// #452 phase C) rather than applied immediately -- see the module doc.
    pub(super) font: FontPreview,

    /// Background image path (`UiConfig::background_image`). Debounced
    /// (issue #452 phase D) like the font fields -- it is a text field with
    /// real filesystem I/O and a GPU texture upload behind it.
    pub(super) background_image_path: Option<PathBuf>,

    /// Shader path (`ShaderConfig::path`). Debounced (issue #452 phase D)
    /// like [`Self::background_image_path`] -- a text field, but backed by a
    /// GLSL compile that can additionally fail; see [`ShaderErrorRoute`].
    pub(super) shader_path: Option<PathBuf>,

    /// Command-block status gutter position (`CommandBlocksConfig::gutter`).
    /// Applied immediately (issue #452 phase D) -- a combo box, not a text
    /// field -- via a direct config write that the existing reactive resize
    /// detection in `app_impl.rs` picks up on its own.
    pub(super) gutter: GutterPosition,

    /// Tab bar position (`TabsConfig::position`). Applied immediately (issue
    /// #452 phase D), chrome layout only, same shape as [`Self::hide_menu_bar`].
    pub(super) tab_bar_position: TabBarPosition,
}

impl PartialEq for VisualPreview {
    /// `background_opacity` and `background_image_opacity` are `f32`;
    /// compare them with an epsilon rather than a derived bitwise `==` so
    /// that two snapshots built from the same logical value (e.g.
    /// round-tripped through TOML) always compare equal. `font` carries its
    /// own epsilon-aware `PartialEq` (size, line height) for the same
    /// reason.
    fn eq(&self, other: &Self) -> bool {
        self.theme_slug == other.theme_slug
            && (self.background_opacity - other.background_opacity).abs() < f32::EPSILON
            && self.profile == other.profile
            && self.cursor == other.cursor
            && self.ligatures == other.ligatures
            && (self.background_image_opacity - other.background_image_opacity).abs() < f32::EPSILON
            && self.background_image_mode == other.background_image_mode
            && self.hide_menu_bar == other.hide_menu_bar
            && self.font == other.font
            && self.background_image_path == other.background_image_path
            && self.shader_path == other.shader_path
            && self.gutter == other.gutter
            && self.tab_bar_position == other.tab_bar_position
    }
}

impl VisualPreview {
    /// Resolve the live-previewable subset of `config`, using `os_dark_mode`
    /// to resolve `ThemeMode::Auto` to a concrete slug.
    pub(super) fn from_config(config: &Config, os_dark_mode: bool) -> Self {
        Self {
            theme_slug: config.theme.active_slug(os_dark_mode).to_string(),
            background_opacity: config.ui.background_opacity,
            profile: config.chrome.profile,
            cursor: CursorPreview {
                shape: config.cursor.shape.clone(),
                blink: config.cursor.blink,
                trail: config.cursor.trail,
                trail_duration_ms: config.cursor.trail_duration_ms,
            },
            ligatures: config.font.ligatures,
            background_image_opacity: config.ui.background_image_opacity,
            background_image_mode: config.ui.background_image_mode,
            hide_menu_bar: config.ui.hide_menu_bar,
            font: FontPreview::from_config(config),
            background_image_path: config.ui.background_image.clone(),
            shader_path: config.shader.path.clone(),
            gutter: config.command_blocks.gutter,
            tab_bar_position: config.tabs.position,
        }
    }

    /// Compute what changed moving from `previous` to `self`, field by
    /// field. Each `Some(_)` is the new value that field should take on;
    /// `None` means that field is unchanged and needs no work.
    ///
    /// Kept pure (no `FreminalGui`, no windowing handle) so the decision of
    /// *what* changed can be unit-tested independently of *how* each change
    /// is carried out (broadcasting `InputEvent::ThemeChange`, writing
    /// `background_opacity`, re-deriving `GuiTheme` -- all of which need a
    /// live `FreminalGui` and are done by `apply_visual_preview` in
    /// `settings_dispatch.rs`).
    pub(super) fn diff_from(&self, previous: &Self) -> VisualPreviewDiff {
        VisualPreviewDiff {
            theme_slug: (self.theme_slug != previous.theme_slug).then(|| self.theme_slug.clone()),
            background_opacity: ((self.background_opacity - previous.background_opacity).abs()
                > f32::EPSILON)
                .then_some(self.background_opacity),
            profile: (self.profile != previous.profile).then_some(self.profile),
            cursor_shape: (self.cursor.shape != previous.cursor.shape)
                .then(|| self.cursor.shape.clone()),
            cursor_blink: (self.cursor.blink != previous.cursor.blink).then_some(self.cursor.blink),
            cursor_trail: (self.cursor.trail != previous.cursor.trail).then_some(self.cursor.trail),
            cursor_trail_duration_ms: (self.cursor.trail_duration_ms
                != previous.cursor.trail_duration_ms)
                .then_some(self.cursor.trail_duration_ms),
            ligatures: (self.ligatures != previous.ligatures).then_some(self.ligatures),
            background_image_opacity: ((self.background_image_opacity
                - previous.background_image_opacity)
                .abs()
                > f32::EPSILON)
                .then_some(self.background_image_opacity),
            background_image_mode: (self.background_image_mode != previous.background_image_mode)
                .then_some(self.background_image_mode),
            hide_menu_bar: (self.hide_menu_bar != previous.hide_menu_bar)
                .then_some(self.hide_menu_bar),
            font_family: (self.font.family != previous.font.family)
                .then(|| FontFamilyChange::from_option(self.font.family.as_deref())),
            font_size: ((self.font.size - previous.font.size).abs() > f32::EPSILON)
                .then_some(self.font.size),
            font_line_height: ((self.font.line_height - previous.font.line_height).abs()
                > f32::EPSILON)
                .then_some(self.font.line_height),
            background_image_path: (self.background_image_path != previous.background_image_path)
                .then(|| {
                    BackgroundImagePathChange::from_option(self.background_image_path.as_deref())
                }),
            shader_path: (self.shader_path != previous.shader_path)
                .then(|| ShaderPathChange::from_option(self.shader_path.as_deref())),
            gutter: (self.gutter != previous.gutter).then_some(self.gutter),
            tab_bar_position: (self.tab_bar_position != previous.tab_bar_position)
                .then_some(self.tab_bar_position),
        }
    }
}

/// The result of [`VisualPreview::diff_from`]: which fields changed between
/// two snapshots, and the new value each one should take on.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct VisualPreviewDiff {
    /// `Some(new_slug)` if the active theme changed.
    pub(super) theme_slug: Option<String>,
    /// `Some(new_opacity)` if the background opacity changed.
    pub(super) background_opacity: Option<f32>,
    /// `Some(new_profile)` if the chrome style profile changed.
    pub(super) profile: Option<StyleProfile>,
    /// `Some(new_shape)` if the cursor shape changed.
    pub(super) cursor_shape: Option<CursorShapeConfig>,
    /// `Some(new_blink)` if the cursor blink toggle changed.
    pub(super) cursor_blink: Option<bool>,
    /// `Some(new_trail)` if the cursor trail toggle changed.
    pub(super) cursor_trail: Option<bool>,
    /// `Some(new_duration_ms)` if the cursor trail duration changed.
    pub(super) cursor_trail_duration_ms: Option<u32>,
    /// `Some(new_ligatures)` if the ligature toggle changed.
    pub(super) ligatures: Option<bool>,
    /// `Some(new_opacity)` if the background image opacity changed.
    pub(super) background_image_opacity: Option<f32>,
    /// `Some(new_mode)` if the background image fit mode changed.
    pub(super) background_image_mode: Option<BackgroundImageMode>,
    /// `Some(new_hidden)` if the "hide menu bar" toggle changed.
    pub(super) hide_menu_bar: Option<bool>,
    /// `Some(change)` if the font family changed; see [`FontFamilyChange`].
    pub(super) font_family: Option<FontFamilyChange>,
    /// `Some(new_size)` if the font size changed.
    pub(super) font_size: Option<f32>,
    /// `Some(new_line_height)` if the line-height multiplier changed.
    pub(super) font_line_height: Option<f32>,
    /// `Some(change)` if the background image path changed; see
    /// [`BackgroundImagePathChange`].
    pub(super) background_image_path: Option<BackgroundImagePathChange>,
    /// `Some(change)` if the shader path changed; see [`ShaderPathChange`].
    pub(super) shader_path: Option<ShaderPathChange>,
    /// `Some(new_gutter)` if the command-block gutter position changed.
    pub(super) gutter: Option<GutterPosition>,
    /// `Some(new_position)` if the tab bar position changed.
    pub(super) tab_bar_position: Option<TabBarPosition>,
}

/// Distinguishes why a [`SettingsAction::Preview`](super::settings::SettingsAction::Preview)
/// carries the [`VisualPreview`] it does (issue #452 phase C).
///
/// [`VisualPreview`] alone cannot make this distinction -- a reverted
/// snapshot and an edited one are both just "a new whole-state snapshot to
/// preview" -- but the font debounce (see [`DebouncedPreview`]) needs it:
/// an in-progress edit should stash an expensive font change and wait, while
/// closing Settings without applying must apply the committed font
/// immediately and discard any pending stash. This is metadata about
/// *timing*, not a second revert mechanism -- see the module doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewTrigger {
    /// The draft changed while Settings is open. Expensive fields go through
    /// the normal debounce.
    Edit,
    /// Settings closed without applying (Cancel, X, or the unsaved-changes
    /// prompt's Discard) -- re-previewing the committed config. Expensive
    /// fields bypass the debounce and apply immediately.
    Revert,
}

/// Where a shader compile/init error observed on the render thread should be
/// surfaced this frame (issue #452 phase D).
///
/// `WindowPostRenderer::last_error` is written inside a `PaintCallback` (no
/// access to `FreminalGui`) and drained once per frame per terminal window
/// in `app_impl.rs`, which has always routed it to a toast on every terminal
/// window. That is the wrong surface for a live *preview*: a user typing a
/// shader path would get a stream of error toasts on every open terminal
/// window before they finish typing a valid one. This type is the decision
/// of which surface a drained error goes to; [`FreminalGui`](super::FreminalGui)
/// stores the current value and updates it every time a shader source is
/// pushed to `WindowPostRenderer::pending_shader` (both the preview and the
/// commit paths in `settings_dispatch.rs`), so a later drain reflects
/// whichever push is actually responsible for the error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum ShaderErrorRoute {
    /// Surface as a toast on every terminal window -- the existing, default
    /// behaviour. Correct for a committed change (Apply / Reload Config),
    /// the initial config load, a `hot_reload` recompile, and a preview
    /// revert (the Settings window is already closing by the time a revert
    /// fires, so its own status message would never be seen).
    #[default]
    Toast,
    /// Surface in the Settings window's own status message instead. Used
    /// only while a shader-path edit is still live and the debounce has just
    /// settled it (`PreviewTrigger::Edit`) -- the window is still open to
    /// show the message, and it must not persist anything or leave a toast
    /// flood behind on every keystroke.
    ///
    /// The settle path (`apply_preview_shader_path_debounced` /
    /// `tick_visual_preview_debounces` in `settings_dispatch.rs`) always
    /// passes `PreviewTrigger::Edit` here, even when the settled value
    /// happens to equal the committed config (e.g. the user typed away and
    /// back again within one debounce window). That is deliberately not
    /// special-cased: while the Settings window is still open, routing to
    /// its own status message is harmless either way, and the only place
    /// this route could otherwise get permanently *stuck* here -- the
    /// window closing before a further edit or genuine revert corrects it
    /// -- is covered unconditionally by [`shader_error_route_on_settings_close`]
    /// at the `app_impl.rs` close site, independent of whatever value this
    /// route held beforehand (issue #452 post-review Blocker 2).
    SettingsStatus,
}

/// Decide the [`ShaderErrorRoute`] that should be in effect immediately
/// after pushing a shader source to `WindowPostRenderer::pending_shader`,
/// given the [`PreviewTrigger`] responsible for the push.
///
/// Pure and GL-context-free by design: the two real call sites
/// (`settings_dispatch.rs`'s shader preview helpers, and `app_impl.rs`'s
/// per-frame `last_error` drain) both need a live `FreminalGui` -- one to
/// push a value, the other to read a compiled result back -- neither of
/// which this decision itself depends on.
pub(super) const fn shader_error_route_for(trigger: PreviewTrigger) -> ShaderErrorRoute {
    match trigger {
        PreviewTrigger::Edit => ShaderErrorRoute::SettingsStatus,
        PreviewTrigger::Revert => ShaderErrorRoute::Toast,
    }
}

/// The [`ShaderErrorRoute`] that must be in effect the instant the Settings
/// window closes, regardless of what it held immediately beforehand.
///
/// This is deliberately unconditional and independent of the debounce
/// holders' own comparisons: `apply_preview_shader_path_immediate` (in
/// `settings_dispatch.rs`) can legitimately early-return without ever
/// calling `push_shader_to_all_windows` -- and therefore without ever
/// calling [`shader_error_route_for`] -- when the debounce baseline already
/// matches the committed shader path. Before this existed, that early
/// return combined with the settle path's
/// `PreviewTrigger::Edit` route left [`ShaderErrorRoute::SettingsStatus`]
/// permanently stuck with the window closed and no visible surface to
/// correct it, silently dropping a later genuine shader hot-reload compile
/// error (issue #452 post-review Blocker 2). Calling this at the
/// `app_impl.rs` settings-window cleanup site -- unconditionally, on every
/// close path (Apply, Cancel, X, Discard) -- makes that impossible: the
/// route can never outlive the session that could have needed the
/// in-window status surface.
pub(super) const fn shader_error_route_on_settings_close() -> ShaderErrorRoute {
    ShaderErrorRoute::Toast
}

/// Decide whether reverting a debounced preview field (font, background
/// image path, or shader path) must actually push `committed` to the
/// renderer/font manager, given the debounce holder's own `applied()`
/// baseline captured *before* [`DebouncedPreview::apply_immediately`] is
/// called, and whether a stash was pending at that same moment.
///
/// This is the decision behind the three `apply_preview_*_immediate`
/// helpers in `settings_dispatch.rs`. It deliberately compares against the
/// debounce holder's own baseline -- what was actually last pushed to the
/// running app -- rather than the draft or `FreminalGui::applied_preview`
/// (the previewable-state snapshot, which tracks every draft edit
/// regardless of whether the debounce has settled it yet). Those two can
/// diverge: a settled preview updates the holder's baseline immediately,
/// but a value typed and then typed back away from before settling updates
/// `applied_preview` on every keystroke while the holder's baseline lags
/// behind. Comparing against the draft-derived baseline is exactly the bug
/// this function exists to avoid (issue #452 post-review Blocker 1):
/// committed `A`, preview `B` and let it settle (baseline is now `B`),
/// retype `A` and cancel before the second debounce settles. The draft is
/// back to `A` (matching committed), but the baseline is still `B`, and the
/// reverted value must still reach the renderer.
///
/// A pending stash always forces a push (even when `previous_applied` and
/// `committed` already match) so that discarding it via `apply_immediately`
/// is always paired with an authoritative push confirming the render
/// pipeline is not left mid-flight on a value the caller has moved on from.
pub(super) fn debounced_revert_needs_push<T: PartialEq>(
    previous_applied: &T,
    committed: &T,
    had_pending: bool,
) -> bool {
    had_pending || previous_applied != committed
}

/// How long an expensive font-preview field (family, size, or line height)
/// must stay unchanged before it is actually applied (issue #452 phase C).
///
/// This trades preview latency against reparse/cache-clear cost: too short
/// and a fast typist entering a font family name still triggers a
/// `FontManager::rebuild` per keystroke (reparsing font files); too long and
/// the live preview feels sluggish once the user stops adjusting. 200ms is
/// comfortably longer than the gap between keystrokes or between two frames
/// of a slider drag, and short enough that the preview settles within a
/// single human-perceptible pause.
pub(super) const FONT_PREVIEW_DEBOUNCE: Duration = Duration::from_millis(200);

/// How long the background-image-path or shader-path preview fields must
/// stay unchanged before they are actually applied (issue #452 phase D).
///
/// Same rationale and interval as [`FONT_PREVIEW_DEBOUNCE`] -- both fields
/// are text fields backed by real filesystem I/O (a GPU texture decode or a
/// GLSL compile) that a fast typist would otherwise re-trigger on every
/// keystroke -- but named separately since the two are different concepts
/// that only happen to share a value.
pub(super) const PATH_PREVIEW_DEBOUNCE: Duration = Duration::from_millis(200);

/// The result of consulting [`debounce_decision`] for one newly observed
/// candidate value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DebounceDecision {
    /// The candidate differs from what is currently pending (or nothing is
    /// pending yet): stash it and reset the "last seen changing" clock.
    Stash,
    /// The candidate matches what is currently pending, but the debounce
    /// interval has not elapsed since it was last seen changing: keep
    /// waiting, no work to do this call.
    Wait,
    /// The candidate matches what is currently pending, and the debounce
    /// interval has elapsed since it was last seen changing: apply it now.
    Apply,
}

/// Pure decision function behind the font-field debounce (issue #452 phase
/// C).
///
/// Takes every input explicitly -- no `FreminalGui`, no settings window, no
/// hidden clock beyond `Instant` arithmetic -- so it is directly
/// unit-testable. `pending` is the value currently stashed and awaiting
/// application (`None` if nothing is stashed); `candidate` is the newly
/// observed value; `since` is when `pending` was last seen changing (`None`
/// iff `pending` is `None`); `now` is the current instant; `interval` is the
/// debounce window.
pub(super) fn debounce_decision<T: PartialEq>(
    pending: Option<&T>,
    candidate: &T,
    since: Option<Instant>,
    now: Instant,
    interval: Duration,
) -> DebounceDecision {
    match (pending, since) {
        (Some(p), Some(t)) if p == candidate => {
            if now.saturating_duration_since(t) >= interval {
                DebounceDecision::Apply
            } else {
                DebounceDecision::Wait
            }
        }
        _ => DebounceDecision::Stash,
    }
}

/// Generic time-based debounce holder for an expensive preview value (issue
/// #452 phase C).
///
/// Tracks two distinct things, both needed by the caller: `applied` is the
/// value most recently and actually pushed to the running app (the baseline
/// a caller diffs a freshly-settled value against, to decide e.g. whether a
/// font change needs `FontManager::rebuild` or just `set_font_size`);
/// `pending`/`since` is the not-yet-applied candidate and when it was last
/// seen changing, per [`debounce_decision`].
///
/// Generic over `T` (rather than hard-coded to [`FontPreview`]) because
/// phase D reuses this unchanged for the background-image path and the
/// shader path, both of which are text fields with real filesystem I/O
/// behind them and the same "don't redo the I/O on every keystroke" need.
#[derive(Debug, Clone)]
pub(super) struct DebouncedPreview<T> {
    applied: T,
    pending: Option<T>,
    since: Option<Instant>,
}

impl<T> DebouncedPreview<T> {
    /// Start a new holder with `applied` as the initial baseline and nothing
    /// pending.
    pub(super) const fn new(applied: T) -> Self {
        Self {
            applied,
            pending: None,
            since: None,
        }
    }

    /// The value most recently and actually applied.
    pub(super) const fn applied(&self) -> &T {
        &self.applied
    }

    /// `true` while a value is stashed and waiting to settle.
    pub(super) const fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Record `value` as immediately and unconditionally applied, discarding
    /// any pending stash without ever applying it.
    ///
    /// Used for the revert path ([`PreviewTrigger::Revert`], which bypasses
    /// the debounce entirely) and right after a full config Apply, so a
    /// stale debounce cannot outlive the session that produced it and apply
    /// itself later over a value the caller has moved on from.
    pub(super) fn apply_immediately(&mut self, value: T) {
        self.applied = value;
        self.pending = None;
        self.since = None;
    }
}

impl<T: Clone + PartialEq> DebouncedPreview<T> {
    /// Take the pending value, if any, clearing the stash and adopting it as
    /// the new `applied` baseline.
    fn take_pending(&mut self) -> Option<T> {
        let value = self.pending.take()?;
        self.since = None;
        self.applied = value.clone();
        Some(value)
    }

    /// Observe a newly changed candidate value (the caller has already
    /// established it differs from something -- typically the previous
    /// frame's candidate, via [`VisualPreviewDiff`]).
    ///
    /// Returns `Some(value)` if [`debounce_decision`] says the debounce has
    /// already settled for this exact candidate (only possible if the
    /// caller calls this again with the same value after the interval has
    /// elapsed); otherwise stashes the candidate and returns `None`.
    pub(super) fn note(&mut self, candidate: T, now: Instant, interval: Duration) -> Option<T> {
        match debounce_decision(self.pending.as_ref(), &candidate, self.since, now, interval) {
            DebounceDecision::Stash => {
                self.pending = Some(candidate);
                self.since = Some(now);
                None
            }
            DebounceDecision::Wait => None,
            DebounceDecision::Apply => self.take_pending(),
        }
    }

    /// Check whether the currently pending value has settled by `now`,
    /// without a new candidate having arrived this call.
    ///
    /// Used on an idle wake ([`request_repaint_after`](freminal_windowing::WindowHandle::request_repaint_after)
    /// scheduled when a value was stashed): once the user stops editing, the
    /// settings draft stops changing and no further
    /// `SettingsAction::Preview` fires on its own, so this is the only way
    /// the debounce ever gets to actually apply a settled value.
    pub(super) fn poll(&mut self, now: Instant, interval: Duration) -> Option<T> {
        let pending = self.pending.clone()?;
        let since = self.since?;
        match debounce_decision(Some(&pending), &pending, Some(since), now, interval) {
            DebounceDecision::Apply => self.take_pending(),
            DebounceDecision::Stash | DebounceDecision::Wait => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BackgroundImagePathChange, DebounceDecision, DebouncedPreview, FontFamilyChange,
        PATH_PREVIEW_DEBOUNCE, PreviewTrigger, ShaderErrorRoute, ShaderPathChange, StyleProfile,
        VisualPreview, debounce_decision, debounced_revert_needs_push, shader_error_route_for,
        shader_error_route_on_settings_close,
    };
    use freminal_common::config::{
        BackgroundImageMode, Config, CursorShapeConfig, GutterPosition, TabBarPosition, ThemeMode,
    };
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn config_with_theme(mode: ThemeMode, dark: &str, light: &str) -> Config {
        let mut cfg = Config::default();
        cfg.theme.mode = mode;
        cfg.theme.dark_name = dark.to_string();
        cfg.theme.light_name = light.to_string();
        cfg
    }

    #[test]
    fn from_config_resolves_dark_mode_directly() {
        let cfg = config_with_theme(ThemeMode::Dark, "nord", "catppuccin-latte");
        let preview = VisualPreview::from_config(&cfg, false);
        assert_eq!(preview.theme_slug, "nord");
    }

    #[test]
    fn from_config_resolves_light_mode_directly() {
        let cfg = config_with_theme(ThemeMode::Light, "nord", "catppuccin-latte");
        let preview = VisualPreview::from_config(&cfg, true);
        assert_eq!(preview.theme_slug, "catppuccin-latte");
    }

    #[test]
    fn from_config_auto_mode_honours_os_dark_mode() {
        let cfg = config_with_theme(ThemeMode::Auto, "nord", "catppuccin-latte");

        let dark = VisualPreview::from_config(&cfg, true);
        assert_eq!(
            dark.theme_slug, "nord",
            "Auto mode with a dark OS must resolve to the dark theme"
        );

        let light = VisualPreview::from_config(&cfg, false);
        assert_eq!(
            light.theme_slug, "catppuccin-latte",
            "Auto mode with a light OS must resolve to the light theme"
        );
    }

    #[test]
    fn from_config_captures_opacity_and_profile() {
        let mut cfg = Config::default();
        cfg.ui.background_opacity = 0.42;
        cfg.chrome.profile = StyleProfile::Retro;
        let preview = VisualPreview::from_config(&cfg, false);
        assert!((preview.background_opacity - 0.42).abs() < f32::EPSILON);
        assert_eq!(preview.profile, StyleProfile::Retro);
    }

    /// Phase B (issue #452): the cheap visual options -- cursor
    /// shape/blink/trail(+duration), ligatures, background image
    /// opacity/mode, and hide-menu-bar -- must all be captured too.
    #[test]
    fn from_config_captures_phase_b_fields() {
        let mut cfg = Config::default();
        cfg.cursor.shape = CursorShapeConfig::Bar;
        cfg.cursor.blink = false;
        cfg.cursor.trail = true;
        cfg.cursor.trail_duration_ms = 400;
        cfg.font.ligatures = false;
        cfg.ui.background_image_opacity = 0.75;
        cfg.ui.background_image_mode = BackgroundImageMode::Tile;
        cfg.ui.hide_menu_bar = true;

        let preview = VisualPreview::from_config(&cfg, false);
        assert_eq!(preview.cursor.shape, CursorShapeConfig::Bar);
        assert!(!preview.cursor.blink);
        assert!(preview.cursor.trail);
        assert_eq!(preview.cursor.trail_duration_ms, 400);
        assert!(!preview.ligatures);
        assert!((preview.background_image_opacity - 0.75).abs() < f32::EPSILON);
        assert_eq!(preview.background_image_mode, BackgroundImageMode::Tile);
        assert!(preview.hide_menu_bar);
    }

    /// Phase C (issue #452): font family/size/line-height must be captured
    /// too.
    #[test]
    fn from_config_captures_phase_c_font_fields() {
        let mut cfg = Config::default();
        cfg.font.family = Some("JetBrains Mono".to_string());
        cfg.font.size = 16.5;
        cfg.font.line_height = 1.2;

        let preview = VisualPreview::from_config(&cfg, false);
        assert_eq!(preview.font.family.as_deref(), Some("JetBrains Mono"));
        assert!((preview.font.size - 16.5).abs() < f32::EPSILON);
        assert!((preview.font.line_height - 1.2).abs() < f32::EPSILON);
    }

    /// Phase D (issue #452): background image path, shader path, gutter
    /// position, and tab bar position must all be captured too.
    #[test]
    fn from_config_captures_phase_d_fields() {
        let mut cfg = Config::default();
        cfg.ui.background_image = Some(PathBuf::from("/tmp/bg.png"));
        cfg.shader.path = Some(PathBuf::from("/tmp/shader.glsl"));
        cfg.command_blocks.gutter = GutterPosition::Off;
        cfg.tabs.position = TabBarPosition::Bottom;

        let preview = VisualPreview::from_config(&cfg, false);
        assert_eq!(
            preview.background_image_path,
            Some(PathBuf::from("/tmp/bg.png"))
        );
        assert_eq!(preview.shader_path, Some(PathBuf::from("/tmp/shader.glsl")));
        assert_eq!(preview.gutter, GutterPosition::Off);
        assert_eq!(preview.tab_bar_position, TabBarPosition::Bottom);
    }

    #[test]
    fn eq_uses_epsilon_for_opacity() {
        let mut a = VisualPreview::from_config(&Config::default(), false);
        let mut b = a.clone();
        a.background_opacity = 0.5;
        // Bit-different but well within float rounding of the same logical
        // value -- must still compare equal.
        b.background_opacity = 0.5 + f32::EPSILON / 2.0;
        assert_eq!(a, b);
    }

    #[test]
    fn eq_uses_epsilon_for_background_image_opacity() {
        let mut a = VisualPreview::from_config(&Config::default(), false);
        let mut b = a.clone();
        a.background_image_opacity = 0.5;
        b.background_image_opacity = 0.5 + f32::EPSILON / 2.0;
        assert_eq!(a, b);
    }

    #[test]
    fn eq_uses_epsilon_for_font_size_and_line_height() {
        // Values kept under 1.0, matching `eq_uses_epsilon_for_opacity` --
        // `f32::EPSILON` is the ULP at 1.0, so a `+ EPSILON / 2.0` nudge is
        // only guaranteed to stay bit-different-but-within-epsilon in the
        // same binade the existing epsilon tests use.  This is purely
        // exercising `FontPreview`'s `PartialEq`, not asserting anything
        // about realistic font sizes.
        let mut a = VisualPreview::from_config(&Config::default(), false);
        let mut b = a.clone();
        a.font.size = 0.5;
        b.font.size = 0.5 + f32::EPSILON / 2.0;
        a.font.line_height = 0.75;
        b.font.line_height = 0.75 + f32::EPSILON / 2.0;
        assert_eq!(a, b);
    }

    #[test]
    fn diff_from_reports_only_changed_fields() {
        let mut before = VisualPreview::from_config(&Config::default(), false);
        before.theme_slug = "catppuccin-mocha".to_string();
        before.background_opacity = 1.0;
        before.profile = StyleProfile::Modern;

        let mut after = before.clone();
        after.background_opacity = 0.5;

        let diff = after.diff_from(&before);
        assert_eq!(diff.theme_slug, None);
        assert_eq!(diff.background_opacity, Some(0.5));
        assert_eq!(diff.profile, None);
        assert_eq!(diff.cursor_shape, None);
        assert_eq!(diff.cursor_blink, None);
        assert_eq!(diff.cursor_trail, None);
        assert_eq!(diff.cursor_trail_duration_ms, None);
        assert_eq!(diff.ligatures, None);
        assert_eq!(diff.background_image_opacity, None);
        assert_eq!(diff.background_image_mode, None);
        assert_eq!(diff.hide_menu_bar, None);
        assert_eq!(diff.font_family, None);
        assert_eq!(diff.font_size, None);
        assert_eq!(diff.font_line_height, None);
        assert_eq!(diff.background_image_path, None);
        assert_eq!(diff.shader_path, None);
        assert_eq!(diff.gutter, None);
        assert_eq!(diff.tab_bar_position, None);
    }

    /// Each phase B field must be reported independently -- changing one
    /// must not mask or be masked by the others.
    #[test]
    fn diff_from_reports_each_phase_b_field_independently() {
        let before = VisualPreview::from_config(&Config::default(), false);

        let mut after = before.clone();
        after.cursor.shape = CursorShapeConfig::Underline;
        after.cursor.blink = !before.cursor.blink;
        after.cursor.trail = !before.cursor.trail;
        after.cursor.trail_duration_ms = before.cursor.trail_duration_ms + 50;
        after.ligatures = !before.ligatures;
        after.background_image_opacity = before.background_image_opacity + 0.1;
        after.background_image_mode = BackgroundImageMode::Tile;
        after.hide_menu_bar = !before.hide_menu_bar;

        let diff = after.diff_from(&before);
        assert_eq!(diff.cursor_shape, Some(CursorShapeConfig::Underline));
        assert_eq!(diff.cursor_blink, Some(after.cursor.blink));
        assert_eq!(diff.cursor_trail, Some(after.cursor.trail));
        assert_eq!(
            diff.cursor_trail_duration_ms,
            Some(after.cursor.trail_duration_ms)
        );
        assert_eq!(diff.ligatures, Some(after.ligatures));
        assert_eq!(
            diff.background_image_opacity,
            Some(after.background_image_opacity)
        );
        assert_eq!(diff.background_image_mode, Some(BackgroundImageMode::Tile));
        assert_eq!(diff.hide_menu_bar, Some(after.hide_menu_bar));
        // Untouched top-level fields must stay unreported.
        assert_eq!(diff.theme_slug, None);
        assert_eq!(diff.background_opacity, None);
        assert_eq!(diff.profile, None);
        assert_eq!(diff.font_family, None);
        assert_eq!(diff.font_size, None);
        assert_eq!(diff.font_line_height, None);
    }

    /// Phase C fields must each be reported independently too, and must not
    /// be masked by (or mask) each other or the phase A/B fields.
    #[test]
    fn diff_from_reports_each_phase_c_font_field_independently() {
        let before = VisualPreview::from_config(&Config::default(), false);

        let mut after = before.clone();
        after.font.family = Some("Fira Code".to_string());
        after.font.size = before.font.size + 2.0;
        after.font.line_height = before.font.line_height + 0.1;

        let diff = after.diff_from(&before);
        assert_eq!(
            diff.font_family,
            Some(FontFamilyChange::Custom("Fira Code".to_string()))
        );
        assert_eq!(diff.font_size, Some(after.font.size));
        assert_eq!(diff.font_line_height, Some(after.font.line_height));
        // Untouched fields must stay unreported.
        assert_eq!(diff.theme_slug, None);
        assert_eq!(diff.ligatures, None);
        assert_eq!(diff.hide_menu_bar, None);
    }

    /// A font family change back to the bundled default (`None`) must be
    /// reported as `Some(FontFamilyChange::Default)`, distinguishable from
    /// "no change" (`None`) -- `FontFamilyChange` exists precisely for this
    /// case.
    #[test]
    fn diff_from_reports_font_family_reset_to_default() {
        let mut before = VisualPreview::from_config(&Config::default(), false);
        before.font.family = Some("Fira Code".to_string());

        let mut after = before.clone();
        after.font.family = None;

        let diff = after.diff_from(&before);
        assert_eq!(diff.font_family, Some(FontFamilyChange::Default));
    }

    #[test]
    fn diff_from_reports_no_changes_when_equal() {
        let preview = VisualPreview::from_config(&Config::default(), false);
        let diff = preview.diff_from(&preview);
        assert_eq!(diff.theme_slug, None);
        assert_eq!(diff.background_opacity, None);
        assert_eq!(diff.profile, None);
        assert_eq!(diff.cursor_shape, None);
        assert_eq!(diff.cursor_blink, None);
        assert_eq!(diff.cursor_trail, None);
        assert_eq!(diff.cursor_trail_duration_ms, None);
        assert_eq!(diff.ligatures, None);
        assert_eq!(diff.background_image_opacity, None);
        assert_eq!(diff.background_image_mode, None);
        assert_eq!(diff.hide_menu_bar, None);
        assert_eq!(diff.font_family, None);
        assert_eq!(diff.font_size, None);
        assert_eq!(diff.font_line_height, None);
        assert_eq!(diff.background_image_path, None);
        assert_eq!(diff.shader_path, None);
        assert_eq!(diff.gutter, None);
        assert_eq!(diff.tab_bar_position, None);
    }

    /// Phase D fields must each be reported independently too, and must not
    /// be masked by (or mask) each other or the phase A/B/C fields.
    #[test]
    fn diff_from_reports_each_phase_d_field_independently() {
        let before = VisualPreview::from_config(&Config::default(), false);

        let mut after = before.clone();
        after.background_image_path = Some(PathBuf::from("/tmp/bg.png"));
        after.shader_path = Some(PathBuf::from("/tmp/shader.glsl"));
        after.gutter = GutterPosition::Off;
        after.tab_bar_position = TabBarPosition::Bottom;

        let diff = after.diff_from(&before);
        assert_eq!(
            diff.background_image_path,
            Some(BackgroundImagePathChange::Custom(PathBuf::from(
                "/tmp/bg.png"
            )))
        );
        assert_eq!(
            diff.shader_path,
            Some(ShaderPathChange::Custom(PathBuf::from("/tmp/shader.glsl")))
        );
        assert_eq!(diff.gutter, Some(GutterPosition::Off));
        assert_eq!(diff.tab_bar_position, Some(TabBarPosition::Bottom));
        // Untouched fields must stay unreported.
        assert_eq!(diff.theme_slug, None);
        assert_eq!(diff.hide_menu_bar, None);
        assert_eq!(diff.font_family, None);
    }

    /// A background image path change back to "no image" (`None`) must be
    /// reported as `Some(BackgroundImagePathChange::Cleared)`,
    /// distinguishable from "no change" (`None`) -- same shape as
    /// `diff_from_reports_font_family_reset_to_default`.
    #[test]
    fn diff_from_reports_background_image_path_reset_to_cleared() {
        let mut before = VisualPreview::from_config(&Config::default(), false);
        before.background_image_path = Some(PathBuf::from("/tmp/bg.png"));

        let mut after = before.clone();
        after.background_image_path = None;

        let diff = after.diff_from(&before);
        assert_eq!(
            diff.background_image_path,
            Some(BackgroundImagePathChange::Cleared)
        );
    }

    /// Same shape again, for the shader path.
    #[test]
    fn diff_from_reports_shader_path_reset_to_cleared() {
        let mut before = VisualPreview::from_config(&Config::default(), false);
        before.shader_path = Some(PathBuf::from("/tmp/shader.glsl"));

        let mut after = before.clone();
        after.shader_path = None;

        let diff = after.diff_from(&before);
        assert_eq!(diff.shader_path, Some(ShaderPathChange::Cleared));
    }

    /// Pins the bug this module's design fixes (issue #452 phase A): a
    /// previewed chrome-profile change had no matching revert, because
    /// `PreviewProfile` wrote `FreminalGui::gui_theme` directly and
    /// `RevertProfile` never existed. Simulate the same sequence purely:
    /// preview a changed profile, then preview the committed config again
    /// (what closing without applying now does) -- the second diff must
    /// report the original profile, restoring it.
    #[test]
    fn reverting_to_committed_config_restores_the_original_profile() {
        let mut committed = Config::default();
        committed.chrome.profile = StyleProfile::Modern;
        let committed_preview = VisualPreview::from_config(&committed, false);

        // The user previews Retro without applying.
        committed.chrome.profile = StyleProfile::Retro;
        let previewed_preview = VisualPreview::from_config(&committed, false);

        let preview_diff = previewed_preview.diff_from(&committed_preview);
        assert_eq!(
            preview_diff.profile,
            Some(StyleProfile::Retro),
            "previewing a profile change must be reported as a profile diff"
        );

        // Closing without applying re-previews the committed config -- the
        // diff must now report a revert back to Modern, not leave Retro
        // applied indefinitely.
        let revert_diff = committed_preview.diff_from(&previewed_preview);
        assert_eq!(
            revert_diff.profile,
            Some(StyleProfile::Modern),
            "reverting to the committed config must restore the original profile"
        );
    }

    /// Same shape as `reverting_to_committed_config_restores_the_original_profile`,
    /// for a phase B field applied by broadcasting `InputEvent::CursorConfigChange`
    /// to the PTY threads: previewing a cursor shape change without applying,
    /// then closing (re-previewing the committed config), must restore the
    /// original shape.
    #[test]
    fn reverting_to_committed_config_restores_the_original_cursor_shape() {
        let mut committed = Config::default();
        committed.cursor.shape = CursorShapeConfig::Block;
        let committed_preview = VisualPreview::from_config(&committed, false);

        // The user previews Bar without applying.
        committed.cursor.shape = CursorShapeConfig::Bar;
        let previewed_preview = VisualPreview::from_config(&committed, false);

        let preview_diff = previewed_preview.diff_from(&committed_preview);
        assert_eq!(
            preview_diff.cursor_shape,
            Some(CursorShapeConfig::Bar),
            "previewing a cursor shape change must be reported as a diff"
        );

        let revert_diff = committed_preview.diff_from(&previewed_preview);
        assert_eq!(
            revert_diff.cursor_shape,
            Some(CursorShapeConfig::Block),
            "reverting to the committed config must restore the original cursor shape"
        );
    }

    /// Same shape again, for a phase B field applied entirely GUI-side (no
    /// PTY round trip): `hide_menu_bar`.
    #[test]
    fn reverting_to_committed_config_restores_the_original_hide_menu_bar_setting() {
        let mut committed = Config::default();
        committed.ui.hide_menu_bar = false;
        let committed_preview = VisualPreview::from_config(&committed, false);

        // The user previews "hidden" without applying.
        committed.ui.hide_menu_bar = true;
        let previewed_preview = VisualPreview::from_config(&committed, false);

        let preview_diff = previewed_preview.diff_from(&committed_preview);
        assert_eq!(
            preview_diff.hide_menu_bar,
            Some(true),
            "previewing hide_menu_bar must be reported as a diff"
        );

        let revert_diff = committed_preview.diff_from(&previewed_preview);
        assert_eq!(
            revert_diff.hide_menu_bar,
            Some(false),
            "reverting to the committed config must restore the original hide_menu_bar value"
        );
    }

    /// Same shape again, for a phase C field: font size. Unlike the phase
    /// A/B fields this one is debounced before it reaches `FontManager`, but
    /// the *diff* reporting itself must still round-trip cleanly -- the
    /// debounce lives entirely in `apply_visual_preview`, not here.
    #[test]
    fn reverting_to_committed_config_restores_the_original_font_size() {
        let mut committed = Config::default();
        committed.font.size = 12.0;
        let committed_preview = VisualPreview::from_config(&committed, false);

        // The user previews a larger size without applying.
        committed.font.size = 18.0;
        let previewed_preview = VisualPreview::from_config(&committed, false);

        let preview_diff = previewed_preview.diff_from(&committed_preview);
        assert_eq!(
            preview_diff.font_size,
            Some(18.0),
            "previewing a font size change must be reported as a diff"
        );

        let revert_diff = committed_preview.diff_from(&previewed_preview);
        assert_eq!(
            revert_diff.font_size,
            Some(12.0),
            "reverting to the committed config must restore the original font size"
        );
    }

    /// Same shape again, for a phase D field applied entirely GUI-side via
    /// the existing reactive resize detection: the command-block gutter
    /// position.
    #[test]
    fn reverting_to_committed_config_restores_the_original_gutter_position() {
        let mut committed = Config::default();
        committed.command_blocks.gutter = GutterPosition::Left;
        let committed_preview = VisualPreview::from_config(&committed, false);

        // The user previews "Off" without applying.
        committed.command_blocks.gutter = GutterPosition::Off;
        let previewed_preview = VisualPreview::from_config(&committed, false);

        let preview_diff = previewed_preview.diff_from(&committed_preview);
        assert_eq!(
            preview_diff.gutter,
            Some(GutterPosition::Off),
            "previewing a gutter position change must be reported as a diff"
        );

        let revert_diff = committed_preview.diff_from(&previewed_preview);
        assert_eq!(
            revert_diff.gutter,
            Some(GutterPosition::Left),
            "reverting to the committed config must restore the original gutter position \
             (and, in `apply_visual_preview`, re-trigger the PTY resize back)"
        );
    }

    /// Same shape again, for a phase D field that is debounced before it
    /// reaches `WindowPostRenderer`: the shader path.
    #[test]
    fn reverting_to_committed_config_restores_the_original_shader_path() {
        let mut committed = Config::default();
        committed.shader.path = Some(PathBuf::from("/tmp/good.glsl"));
        let committed_preview = VisualPreview::from_config(&committed, false);

        // The user previews a different (possibly broken) shader without
        // applying.
        committed.shader.path = Some(PathBuf::from("/tmp/broken.glsl"));
        let previewed_preview = VisualPreview::from_config(&committed, false);

        let preview_diff = previewed_preview.diff_from(&committed_preview);
        assert_eq!(
            preview_diff.shader_path,
            Some(ShaderPathChange::Custom(PathBuf::from("/tmp/broken.glsl"))),
            "previewing a shader path change must be reported as a diff"
        );

        let revert_diff = committed_preview.diff_from(&previewed_preview);
        assert_eq!(
            revert_diff.shader_path,
            Some(ShaderPathChange::Custom(PathBuf::from("/tmp/good.glsl"))),
            "reverting to the committed config must restore the original shader path"
        );
    }

    // ── Shader compile-error routing (issue #452 phase D) ─────────────────

    #[test]
    fn shader_error_route_for_edit_trigger_is_settings_status() {
        assert_eq!(
            shader_error_route_for(PreviewTrigger::Edit),
            ShaderErrorRoute::SettingsStatus,
            "a live in-progress edit must route a compile failure to the \
             Settings window's own status message, not a toast"
        );
    }

    #[test]
    fn shader_error_route_for_revert_trigger_is_toast() {
        assert_eq!(
            shader_error_route_for(PreviewTrigger::Revert),
            ShaderErrorRoute::Toast,
            "a revert closes the Settings window, so its status message \
             would never be seen -- must fall back to the toast"
        );
    }

    #[test]
    fn shader_error_route_default_is_toast() {
        // Committed changes (Apply / Reload Config) and the initial config
        // load never go through `shader_error_route_for` at all -- they must
        // still land on the toast via the plain `Default` impl.
        assert_eq!(ShaderErrorRoute::default(), ShaderErrorRoute::Toast);
    }

    // ── Font-field debounce (issue #452 phase C) ──────────────────────────

    const INTERVAL: Duration = Duration::from_millis(200);

    #[test]
    fn debounce_decision_stashes_a_first_observed_change() {
        let now = Instant::now();
        let decision = debounce_decision(None, &14.0_f32, None, now, INTERVAL);
        assert_eq!(decision, DebounceDecision::Stash);
    }

    #[test]
    fn debounce_decision_restashes_a_further_change_within_the_interval() {
        let stashed_at = Instant::now();
        let now = stashed_at + Duration::from_millis(50);
        // A different candidate arrives before the interval elapses.
        let decision =
            debounce_decision(Some(&14.0_f32), &15.0_f32, Some(stashed_at), now, INTERVAL);
        assert_eq!(
            decision,
            DebounceDecision::Stash,
            "a further change within the interval must re-stash, not apply"
        );
    }

    #[test]
    fn debounce_decision_waits_when_unchanged_but_interval_not_elapsed() {
        let stashed_at = Instant::now();
        let now = stashed_at + Duration::from_millis(50);
        let decision =
            debounce_decision(Some(&14.0_f32), &14.0_f32, Some(stashed_at), now, INTERVAL);
        assert_eq!(decision, DebounceDecision::Wait);
    }

    #[test]
    fn debounce_decision_applies_once_settled_for_the_full_interval() {
        let stashed_at = Instant::now();
        let now = stashed_at + INTERVAL;
        let decision =
            debounce_decision(Some(&14.0_f32), &14.0_f32, Some(stashed_at), now, INTERVAL);
        assert_eq!(
            decision,
            DebounceDecision::Apply,
            "the value must apply once the interval has elapsed with no further change"
        );
    }

    #[test]
    fn debounced_preview_note_stashes_then_applies_after_settling() {
        let mut holder = DebouncedPreview::new(12.0_f32);
        let t0 = Instant::now();

        assert_eq!(
            holder.note(14.0, t0, INTERVAL),
            None,
            "a changed value must stash, not apply immediately"
        );
        assert!(
            (*holder.applied() - 12.0).abs() < f32::EPSILON,
            "the baseline must not move yet"
        );
        assert!(holder.is_pending());

        // A further, different edit within the interval re-stashes.
        let t1 = t0 + Duration::from_millis(50);
        assert_eq!(holder.note(16.0, t1, INTERVAL), None);
        assert!((*holder.applied() - 12.0).abs() < f32::EPSILON);

        // No further edits; poll after the interval has elapsed since the
        // *second* stash (t1) -- this is what the scheduled
        // `request_repaint_after` wake drives.
        let t2 = t1 + INTERVAL;
        assert_eq!(
            holder.poll(t2, INTERVAL),
            Some(16.0),
            "the settled value must apply once the interval elapses"
        );
        assert!(
            (*holder.applied() - 16.0).abs() < f32::EPSILON,
            "the baseline must adopt the settled value"
        );
        assert!(!holder.is_pending());
    }

    #[test]
    fn debounced_preview_poll_returns_none_before_settling() {
        let mut holder = DebouncedPreview::new(12.0_f32);
        let t0 = Instant::now();
        holder.note(14.0, t0, INTERVAL);

        let too_soon = t0 + Duration::from_millis(50);
        assert_eq!(holder.poll(too_soon, INTERVAL), None);
        assert!((*holder.applied() - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn debounced_preview_poll_returns_none_when_nothing_pending() {
        let mut holder = DebouncedPreview::new(12.0_f32);
        assert_eq!(holder.poll(Instant::now(), INTERVAL), None);
    }

    /// A revert bypasses the debounce entirely and clears any pending stash
    /// -- `apply_immediately` is what `PreviewTrigger::Revert` calls.
    #[test]
    fn debounced_preview_apply_immediately_bypasses_debounce_and_clears_pending() {
        let mut holder = DebouncedPreview::new(12.0_f32);
        let t0 = Instant::now();
        holder.note(20.0, t0, INTERVAL);
        assert!(holder.is_pending(), "a value must be stashed mid-drag");

        holder.apply_immediately(12.0);
        assert!(
            (*holder.applied() - 12.0).abs() < f32::EPSILON,
            "revert must apply the committed value immediately"
        );
        assert!(
            !holder.is_pending(),
            "revert must discard the pending stash, not just leave it to apply later"
        );

        // The stashed value must never surface later, even past the
        // interval.
        let later = t0 + INTERVAL + Duration::from_millis(1);
        assert_eq!(
            holder.poll(later, INTERVAL),
            None,
            "a discarded stash must never apply itself after the fact"
        );
    }

    // ── Debounce holder reuse for path fields (issue #452 phase D) ────────

    /// `DebouncedPreview<T>` must be reused unchanged for `Option<PathBuf>`
    /// (the background image and shader path fields) rather than a second,
    /// bespoke debounce being written for them -- exactly what the type's
    /// own module doc says phase D does. Exercises the same revert shape as
    /// `debounced_preview_apply_immediately_bypasses_debounce_and_clears_pending`,
    /// but with a wholly different, non-`Copy`, non-numeric `T` to prove the
    /// holder is genuinely generic rather than only ever instantiated with
    /// `f32`.
    #[test]
    fn debounced_preview_reused_for_path_types_discards_stash_on_revert() {
        let mut holder: DebouncedPreview<Option<PathBuf>> = DebouncedPreview::new(None);
        let t0 = Instant::now();

        let candidate = Some(PathBuf::from("/tmp/shader.glsl"));
        assert_eq!(
            holder.note(candidate, t0, PATH_PREVIEW_DEBOUNCE),
            None,
            "a changed path must stash, not apply immediately"
        );
        assert!(holder.is_pending());
        assert_eq!(*holder.applied(), None, "the baseline must not move yet");

        // Revert bypasses the debounce entirely and discards the stash,
        // exactly as it does for the font fields.
        holder.apply_immediately(None);
        assert_eq!(
            *holder.applied(),
            None,
            "revert must apply the committed (cleared) path immediately"
        );
        assert!(
            !holder.is_pending(),
            "revert must discard the pending path stash, not just leave it to apply later"
        );

        // The discarded stash must never surface later, even past the
        // interval.
        let later = t0 + PATH_PREVIEW_DEBOUNCE + Duration::from_millis(1);
        assert_eq!(
            holder.poll(later, PATH_PREVIEW_DEBOUNCE),
            None,
            "a discarded path stash must never apply itself after the fact"
        );
    }

    // ── Close-path revert correctness (issue #452 post-review fixes) ──────
    //
    // The review that produced these fixes found the settings.rs-level gate
    // (`committed_preview != preview_before`) untestable in isolation
    // because it lives inside `show()` / `show_standalone()`, which need a
    // live `egui::Context` to exercise at all -- no existing test in this
    // module or `settings.rs` renders those functions. The two blockers it
    // caused are pinned here instead, at the pure-function level Fix 2 and
    // Fix 3 pushed the actual decisions down to:
    // `debounced_revert_needs_push` (used by the three
    // `apply_preview_*_immediate` helpers in `settings_dispatch.rs`) and
    // `shader_error_route_on_settings_close` (used by the `app_impl.rs`
    // settings-window cleanup site). `FreminalGui` itself cannot be
    // constructed headlessly (no test anywhere in `app_impl.rs` or
    // `settings_dispatch.rs` instantiates one; its own test module only
    // tests free functions), so this is the right level.

    #[test]
    fn debounced_revert_needs_push_true_when_baseline_diverged_even_if_committed_matches_draft() {
        // Pins Blocker 1's central claim: even though `committed` is what
        // the draft now shows (the old settings.rs gate would have called
        // this "nothing changed" and skipped emitting a revert at all), the
        // debounce holder's own baseline has already diverged from it and a
        // push is still required.
        let previous_applied = "B".to_string();
        let committed = "A".to_string();
        assert!(
            debounced_revert_needs_push(&previous_applied, &committed, false),
            "a diverged baseline must require a push even with nothing pending"
        );
    }

    #[test]
    fn debounced_revert_needs_push_false_when_baseline_already_matches_and_nothing_pending() {
        // Control case: the common, overwhelmingly frequent path where the
        // renderer already reflects the committed value and there is
        // nothing in flight -- must stay a no-op.
        let value = "A".to_string();
        assert!(
            !debounced_revert_needs_push(&value, &value, false),
            "an already-matching baseline with nothing pending needs no push"
        );
    }

    #[test]
    fn debounced_revert_needs_push_true_when_pending_even_if_baseline_matches() {
        // A pending stash must always force a push, even when the baseline
        // already happens to equal the committed value -- discarding the
        // stash via `apply_immediately` must always be paired with an
        // authoritative push, never silently left as a bare discard.
        let value = "A".to_string();
        assert!(
            debounced_revert_needs_push(&value, &value, true),
            "a pending stash must force a push regardless of the baseline"
        );
    }

    /// Full reproduction of Blocker 1's sequence at the debounce-holder
    /// level: committed shader `A`, preview `B` and let it settle for real
    /// (the holder's baseline becomes `B`), then retype `A` (back to
    /// committed) but cancel before the second debounce settles, leaving a
    /// pending stash. The committed value must still be recognised as
    /// needing a push, and the pending stash must be discarded synchronously
    /// -- both are Fix 2's requirements on the three
    /// `apply_preview_*_immediate` helpers, exercised here through the exact
    /// sequence (`applied()`/`is_pending()` captured *before*
    /// `apply_immediately`, matching the real call sites in
    /// `settings_dispatch.rs`).
    #[test]
    fn revert_of_a_settled_debounce_still_pushes_when_baseline_diverged_from_committed() {
        let mut holder = DebouncedPreview::new(Some(PathBuf::from("/tmp/a.glsl")));
        let t0 = Instant::now();

        // Preview B and let it settle for real.
        holder.note(
            Some(PathBuf::from("/tmp/b.glsl")),
            t0,
            PATH_PREVIEW_DEBOUNCE,
        );
        let settled = holder.poll(t0 + PATH_PREVIEW_DEBOUNCE, PATH_PREVIEW_DEBOUNCE);
        assert_eq!(settled, Some(Some(PathBuf::from("/tmp/b.glsl"))));
        assert_eq!(*holder.applied(), Some(PathBuf::from("/tmp/b.glsl")));

        // Retype A (back to committed) but don't wait for it to settle.
        let t1 = t0 + PATH_PREVIEW_DEBOUNCE + Duration::from_millis(10);
        holder.note(
            Some(PathBuf::from("/tmp/a.glsl")),
            t1,
            PATH_PREVIEW_DEBOUNCE,
        );
        assert!(
            holder.is_pending(),
            "the retyped-but-not-yet-settled value must be stashed"
        );

        // Cancel: this is exactly what `apply_preview_shader_path_immediate`
        // (and its font / background-image siblings) does.
        let committed = Some(PathBuf::from("/tmp/a.glsl"));
        let previous_applied = holder.applied().clone();
        let had_pending = holder.is_pending();
        holder.apply_immediately(committed.clone());

        assert!(
            debounced_revert_needs_push(&previous_applied, &committed, had_pending),
            "the baseline (B) diverged from the committed value (A); revert \
             must still push even though the draft already matched \
             committed and the pre-fix settings.rs gate would have skipped \
             emitting a revert at all"
        );
        assert!(
            !holder.is_pending(),
            "the pending stash must be discarded synchronously by \
             `apply_immediately`, not merely superseded"
        );
        assert_eq!(
            *holder.applied(),
            committed,
            "the holder's baseline must now be the committed value"
        );
    }

    /// Pins Blocker 2: a shader-path settle while editing routes compile
    /// errors to the Settings window's own status message
    /// (`ShaderErrorRoute::SettingsStatus`); closing the window afterwards
    /// -- even via a revert that itself has nothing to push (per
    /// `debounced_revert_needs_push` returning `false`, in which case
    /// `apply_preview_shader_path_immediate` never calls
    /// `shader_error_route_for` again) -- must still land back on
    /// `ShaderErrorRoute::Toast`, because `shader_error_route_on_settings_close`
    /// is unconditional and does not depend on what the route held before.
    #[test]
    fn shader_error_route_after_settle_then_close_is_always_toast() {
        // Simulate the settle-while-editing step.
        let after_settle = shader_error_route_for(PreviewTrigger::Edit);
        assert_eq!(after_settle, ShaderErrorRoute::SettingsStatus);

        // Simulate the settings window closing -- unconditionally, ignoring
        // `after_settle` entirely.
        let after_close = shader_error_route_on_settings_close();
        assert_eq!(
            after_close,
            ShaderErrorRoute::Toast,
            "the route must always land back on Toast when the window \
             closes, regardless of whatever it held immediately before, so \
             a later genuine compile failure is never silently dropped \
             into an invisible, closed status message"
        );
    }

    #[test]
    fn shader_error_route_on_settings_close_is_toast_even_starting_from_toast() {
        // The function must be a true unconditional reset, not a
        // conditional one that happens to look unconditional only when
        // starting from `SettingsStatus`.
        let _ = shader_error_route_for(PreviewTrigger::Revert); // already Toast
        assert_eq!(
            shader_error_route_on_settings_close(),
            ShaderErrorRoute::Toast
        );
    }
}
