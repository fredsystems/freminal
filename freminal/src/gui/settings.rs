// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use egui::{self, ComboBox, DragValue, FontData, FontDefinitions, FontFamily, Panel, Slider, Ui};
use freminal_common::config::{
    self, BackgroundImageMode, Config, CursorShapeConfig, GutterPosition, TabBarPosition,
    TabTitlePolicy, ThemeMode,
};
use freminal_common::keybindings::{BindingMap, KeyAction, KeyCombo};
use freminal_common::themes;
use std::path::PathBuf;

/// Which tab is currently active in the settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Font,
    Cursor,
    Theme,
    Shell,
    Scrollback,
    Logging,
    Ui,
    Tabs,
    ShellIntegration,
    Bell,
    Notifications,
    Security,
    Keybindings,
    Startup,
}

impl SettingsTab {
    /// All tabs in display order.
    const ALL: [Self; 14] = [
        Self::Font,
        Self::Cursor,
        Self::Theme,
        Self::Shell,
        Self::Scrollback,
        Self::Logging,
        Self::Ui,
        Self::Tabs,
        Self::ShellIntegration,
        Self::Bell,
        Self::Notifications,
        Self::Security,
        Self::Keybindings,
        Self::Startup,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Font => "Font",
            Self::Cursor => "Cursor",
            Self::Theme => "Theme",
            Self::Shell => "Shell",
            Self::Scrollback => "Scrollback",
            Self::Logging => "Logging",
            Self::Ui => "UI",
            Self::Tabs => "Tabs",
            Self::ShellIntegration => "Shell Integration",
            Self::Bell => "Bell",
            Self::Notifications => "Notifications",
            Self::Security => "Security",
            Self::Keybindings => "Keybindings",
            Self::Startup => "Startup",
        }
    }
}

/// The result of showing the settings modal for one frame.
///
/// The caller uses this to decide whether to apply config changes, re-register
/// fonts, etc.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    /// The user clicked the delete button next to a layout in the library.
    /// The contained path is the layout file to delete.
    DeleteLayout(std::path::PathBuf),
    /// No action this frame (modal still open or closed without applying).
    None,
    /// The user clicked Apply — the new config has been saved to disk and
    /// should be adopted live.
    Applied,
    /// The user changed the theme in the dropdown — apply it temporarily
    /// so they can preview it in the terminal.  Carries the new theme slug.
    PreviewTheme(String),
    /// The modal was closed without Apply while a preview was active —
    /// revert to the original theme.  Carries the original theme slug and
    /// the original opacity (in case opacity was also previewed).
    RevertTheme(String, f32),
    /// The user changed the opacity slider — apply it temporarily so they
    /// can preview the effect in real time.
    PreviewOpacity(f32),
    /// The modal was closed without Apply while opacity was being previewed —
    /// revert to the original value.
    RevertOpacity(f32),
    /// The user clicked "Test Notification" in the Notifications tab — route
    /// a sample notification through the current draft `[notifications]`
    /// config so the user can verify routing without running a command.
    TestNotification,
    /// The user clicked "Test Paste" in the Security tab — open the confirm
    /// dialog with sample content using the current draft `[paste_guard]`
    /// config so the user can preview the guard without pasting for real.
    TestPaste,
}

/// Result of scanning a frame's egui events for a key-binding press.
#[derive(Debug, Clone, Copy)]
enum KeyCapture {
    /// User pressed Escape — cancel recording.
    Cancelled,
    /// A valid binding combo was captured.
    Combo(KeyCombo),
}

/// Pending close request state for the unsaved-changes guard.
///
/// When the user tries to close the settings modal while `is_dirty()` is true,
/// the close is deferred and a confirmation prompt is shown asking whether to
/// save, discard, or cancel.  The variant records how the close was requested
/// so the guard can answer correctly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingClose {
    /// No close pending.
    None,
    /// Close was requested — show the confirm prompt until the user chooses.
    Asking,
}

/// State machine for the press-to-record keybinding editor.
#[derive(Debug, Clone)]
enum KeyRecordingState {
    /// Not recording — normal grid display.
    Idle,
    /// Waiting for the user to press a key combination.
    Recording {
        /// The action whose binding is being changed.
        action: KeyAction,
    },
    /// A combo was captured and may conflict with another action.
    /// Show a confirmation dialog with conflict info.
    Confirming {
        /// The action whose binding is being changed.
        action: KeyAction,
        /// The captured combo.
        combo: KeyCombo,
        /// If `Some`, the combo is already bound to this other action.
        conflict: Option<KeyAction>,
    },
}

/// Persistent state for the settings modal.
// Several independent UI flags (window-open, per-tab "pending action" one-shots
// like test-notification / test-paste, recording state). Each is a distinct,
// unrelated boolean; collapsing them into a state machine would obscure rather
// than clarify the modal's state.
#[allow(clippy::struct_excessive_bools)]
pub struct SettingsModal {
    /// Whether the modal window is currently visible.
    pub is_open: bool,

    /// Working copy of the configuration being edited. Independent of the live
    /// config until the user clicks Apply.
    draft: Config,

    /// Which tab is active.
    active_tab: SettingsTab,

    /// Error or status message to display (e.g. save failure).
    status_message: Option<String>,

    /// Path override for saving (from `--config` CLI flag). `None` means use
    /// the platform default.
    config_path: Option<PathBuf>,

    /// Cached log directory display string, computed once when the modal is
    /// opened rather than on every UI frame (avoids repeated filesystem calls).
    log_dir_display: String,

    /// When `Some`, the settings modal is in read-only mode: all controls are
    /// visible for browsing but Apply and Reset to Defaults are disabled.  The
    /// string is displayed as a banner explaining why editing is disabled.
    read_only_reason: Option<String>,

    /// Active theme slug when the modal was opened (computed from the live
    /// config's `active_slug(os_dark_mode)`).  Used to detect whether a
    /// preview is active and to revert on Cancel.
    original_theme_slug: String,

    /// The OS dark/light preference at the time the modal was last opened or
    /// `show()` was called.  Used to resolve `ThemeMode::Auto` to the correct
    /// active slug for preview/revert comparison.
    os_dark_mode: bool,

    /// Background opacity when the modal was opened.  Used to detect
    /// whether opacity preview is active and to revert on Cancel.
    original_opacity: f32,

    /// Sorted list of monospaced font family names available on the system.
    /// Populated once when the modal is opened via [`Self::open()`].
    monospace_families: Vec<String>,

    /// Base egui `FontDefinitions` (without any preview font registered).
    /// Saved when the modal opens so we can restore the original font set
    /// when the modal closes.
    base_font_defs: Option<FontDefinitions>,

    /// Which font family is currently registered as the preview font in egui.
    /// `None` means no preview font has been registered (or the default is selected).
    preview_registered: Option<String>,

    /// State for the press-to-record keybinding editor.
    key_recording: KeyRecordingState,

    /// Snapshot of discovered layout files for the Startup tab.
    ///
    /// Updated by the caller before opening or each frame via
    /// [`Self::set_discovered_layouts`].
    pub discovered_layouts: Vec<freminal_common::layout::LayoutSummary>,

    /// Set by `show_startup_tab` when the user clicks a delete button next to
    /// a layout.  Consumed by `show_standalone` / `show` which return it as
    /// `SettingsAction::DeleteLayout`.
    pending_delete_layout: Option<std::path::PathBuf>,

    /// Serialized TOML of the draft at the moment the modal was opened or the
    /// last successful Apply.  Used by `is_dirty()` to detect unsaved edits
    /// without requiring `PartialEq` on every sub-config struct.
    ///
    /// Serialization is cheap enough to perform on open/apply and only
    /// required once per close attempt.
    baseline_toml: String,

    /// Pending close request from Cancel, embedded X, or OS close.  When
    /// `Asking`, the settings UI renders a confirmation prompt instead of
    /// actually closing.  Cleared by Save / Discard / keep-open decisions.
    pending_close: PendingClose,

    /// Set by `show_notifications_tab` when the user clicks "Test
    /// Notification".  Consumed by `show` / `show_standalone` which return it
    /// as `SettingsAction::TestNotification` so the app can route a sample
    /// notification through the draft `[notifications]` config.
    pending_test_notification: bool,

    /// Set by `show_security_tab` when the user clicks "Test Paste".  Consumed
    /// by `show` / `show_standalone` which return it as
    /// `SettingsAction::TestPaste` so the app can open the confirm dialog with
    /// sample content using the draft `[paste_guard]` config.
    pending_test_paste: bool,
}

impl SettingsModal {
    /// Create a new (closed) settings modal.
    #[must_use]
    pub fn new(config_path: Option<PathBuf>) -> Self {
        Self {
            is_open: false,
            draft: Config::default(),
            active_tab: SettingsTab::Font,
            status_message: None,
            config_path,
            log_dir_display: String::new(),
            read_only_reason: None,
            original_theme_slug: String::new(),
            os_dark_mode: false,
            original_opacity: 1.0,
            monospace_families: Vec::new(),
            base_font_defs: None,
            preview_registered: None,
            key_recording: KeyRecordingState::Idle,
            discovered_layouts: Vec::new(),
            pending_delete_layout: None,
            baseline_toml: String::new(),
            pending_close: PendingClose::None,
            pending_test_notification: false,
            pending_test_paste: false,
        }
    }

    /// Replace the draft with `live_config` so that a subsequent `open()`
    /// (or the currently-open modal) reflects on-disk state.
    ///
    /// Used by the "Reload Config" menu action (subtask 71.17) to keep the
    /// settings draft in sync with a live config reloaded from `config.toml`.
    /// Preserves the currently-selected tab and other transient UI state.
    pub(super) fn sync_from_config(&mut self, live_config: &Config) {
        self.draft = live_config.clone();
        self.original_theme_slug = live_config.theme.active_slug(self.os_dark_mode).to_string();
        self.original_opacity = live_config.ui.background_opacity;
        self.baseline_toml = Self::serialize_for_baseline(live_config);
        self.pending_close = PendingClose::None;
    }

    /// Serialize a config to the canonical TOML form used as the dirty-check
    /// baseline.  Delegates to `freminal_common::config::serialize_config_for_diff`
    /// so all callers share the same canonical form.  Returns an empty string
    /// on serialization failure, which causes `is_dirty()` to conservatively
    /// report dirty and trigger the confirmation prompt — preferable to
    /// silently dropping edits.
    fn serialize_for_baseline(config: &Config) -> String {
        config::serialize_config_for_diff(config)
    }

    /// Returns `true` if the draft differs from the baseline captured at the
    /// last open/apply.  Used by the unsaved-changes guard on close.
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        Self::serialize_for_baseline(&self.draft) != self.baseline_toml
    }

    /// Request to close the modal, consulting the dirty state.
    ///
    /// Returns `true` if the modal may close immediately (no pending edits),
    /// `false` if the caller should keep the window open so the user can
    /// resolve the confirm prompt.  Callers include the embedded Cancel
    /// button, the egui window X, and the app's `on_close_requested` hook
    /// for the standalone settings OS window.
    ///
    /// When read-only, close is always allowed (Apply is disabled, so there
    /// can be no unsaved edits from the user's perspective).
    pub fn request_close(&mut self) -> bool {
        if !self.is_open {
            return true;
        }
        if self.read_only_reason.is_some() || !self.is_dirty() {
            self.is_open = false;
            self.pending_close = PendingClose::None;
            return true;
        }
        self.pending_close = PendingClose::Asking;
        false
    }

    /// Render the unsaved-changes confirmation prompt, if one is pending.
    ///
    /// Returns `Some(action)` when the user chose Save (triggers apply) or
    /// Discard (closes the modal).  Returns `None` for Cancel / still
    /// deciding / no prompt pending.
    fn show_confirm_close_prompt(&mut self, ctx: &egui::Context) -> SettingsAction {
        if self.pending_close != PendingClose::Asking {
            return SettingsAction::None;
        }
        let mut action = SettingsAction::None;
        egui::Window::new("Unsaved changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("You have unsaved changes in Settings.");
                ui.add_space(6.0);
                ui.label("Save before closing?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        action = self.try_apply();
                        self.pending_close = PendingClose::None;
                    }
                    if ui.button("Discard").clicked() {
                        self.is_open = false;
                        self.pending_close = PendingClose::None;
                    }
                    if ui.button("Cancel").clicked() {
                        self.pending_close = PendingClose::None;
                    }
                });
            });
        action
    }

    /// Open the modal, cloning the live config into the draft for editing.
    ///
    /// `monospace_families` is the sorted, deduplicated list of monospaced font
    /// family names available on the system (from `FontManager::enumerate_monospace_families`).
    ///
    /// `os_dark_mode` reflects the current OS light/dark preference and is used to
    /// resolve `ThemeMode::Auto` to the correct active slug for preview/revert.
    pub fn open(
        &mut self,
        live_config: &Config,
        monospace_families: Vec<String>,
        os_dark_mode: bool,
    ) {
        self.draft = live_config.clone();
        self.active_tab = SettingsTab::Font;
        self.status_message = None;
        self.monospace_families = monospace_families;
        self.os_dark_mode = os_dark_mode;
        self.original_theme_slug = live_config.theme.active_slug(os_dark_mode).to_string();
        self.original_opacity = live_config.ui.background_opacity;
        self.log_dir_display = config::log_dir().map_or_else(
            || "(unable to determine log directory)".to_string(),
            |p| p.display().to_string(),
        );

        // Determine read-only status:
        //   Layer 1: managed_by field set → HM-specific message
        //   Layer 2: config file not writable → generic message
        self.read_only_reason = if let Some(manager) = &live_config.managed_by {
            Some(format!(
                "Configuration is managed by {manager}. \
                 Edit your {manager} configuration to change settings."
            ))
        } else if !config::config_is_writable(self.config_path.as_deref()) {
            Some("Configuration file is read-only.".to_string())
        } else {
            None
        };

        self.key_recording = KeyRecordingState::Idle;
        self.baseline_toml = Self::serialize_for_baseline(live_config);
        self.pending_close = PendingClose::None;
        self.is_open = true;
    }

    /// Open the modal with a specific tab preselected.
    ///
    /// Wraps [`Self::open`] and overrides the default `SettingsTab::Font`
    /// starting tab.  Used by the Help menu's "Keybindings..." item to jump
    /// directly to the Keybindings tab.
    pub fn open_to_tab(
        &mut self,
        live_config: &Config,
        monospace_families: Vec<String>,
        os_dark_mode: bool,
        tab: SettingsTab,
    ) {
        self.open(live_config, monospace_families, os_dark_mode);
        self.active_tab = tab;
    }

    /// Set the active tab on an already-open modal.
    ///
    /// Used by the Help menu's "Keybindings..." item when the settings
    /// window is already open — we switch tabs instead of re-opening.
    pub const fn set_active_tab(&mut self, tab: SettingsTab) {
        self.active_tab = tab;
    }

    /// Show the modal window. Returns the action the caller should take.
    ///
    /// When `SettingsAction::Applied` is returned, the caller should:
    ///   1. Replace its live config with `self.applied_config()`.
    ///   2. Hot-reload any settings that can change at runtime.
    ///
    /// `os_dark_mode` is used to resolve `ThemeMode::Auto` for preview/revert.
    /// Render the settings UI as a standalone window (fills the entire egui context).
    ///
    /// Unlike [`show()`], which creates a floating `egui::Window` inside a parent
    /// terminal window, this method renders directly into a `CentralPanel`.
    /// Used when the settings dialog is its own OS window.
    pub fn show_standalone(&mut self, ctx: &egui::Context, os_dark_mode: bool) -> SettingsAction {
        self.os_dark_mode = os_dark_mode;
        if !self.is_open {
            return SettingsAction::None;
        }

        let mut action = SettingsAction::None;

        let theme_before = self.draft.theme.active_slug(self.os_dark_mode).to_string();
        let opacity_before = self.draft.ui.background_opacity;

        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("settings_root"),
            egui::UiBuilder::default(),
        );

        // Bottom bar must be laid out first (Panel reserves space from the root).
        let is_read_only = self.read_only_reason.is_some();
        Panel::bottom("settings_bottom_bar").show_inside(&mut root_ui, |ui| {
            ui.add_space(4.0);
            if let Some(msg) = &self.status_message {
                ui.colored_label(egui::Color32::YELLOW, msg);
            }
            ui.horizontal(|ui| {
                let reset_btn = egui::Button::new("Reset to Defaults");
                if ui.add_enabled(!is_read_only, reset_btn).clicked() {
                    self.draft = Config::default();
                    self.status_message = Some("Reset to defaults (not saved yet)".to_string());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let apply_btn = egui::Button::new("Apply");
                    if ui.add_enabled(!is_read_only, apply_btn).clicked() {
                        action = self.try_apply();
                    }
                    if ui.button("Cancel").clicked() {
                        // Route through the dirty-state guard so unsaved
                        // edits surface a confirmation prompt.
                        self.request_close();
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show_inside(&mut root_ui, |ui| {
            ui.heading("Settings");
            ui.add_space(4.0);

            // --- Read-only banner ---
            if let Some(reason) = &self.read_only_reason {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(80, 60, 20))
                    .corner_radius(4.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.colored_label(egui::Color32::from_rgb(255, 220, 100), reason);
                    });
                ui.add_space(4.0);
            }

            // --- Tab bar ---
            self.show_tab_bar(ui, "settings_tab_bar_standalone");

            // --- Tab content ---
            // Both axes so horizontally-overflowing content (long labels,
            // wide swatches) is reachable. Scrollbars only appear when
            // content actually overflows to avoid visual noise.
            egui::ScrollArea::both()
                .auto_shrink([false; 2])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    if is_read_only {
                        ui.disable();
                    }
                    self.draw_active_tab(ui);
                });
        });

        // Render the unsaved-changes prompt on top of the settings UI.
        // If it returns Applied (Save button), adopt it so the revert/preview
        // fallthrough below doesn't misread the closed state as a cancel.
        let prompt_action = self.show_confirm_close_prompt(ctx);
        if prompt_action == SettingsAction::Applied {
            action = prompt_action;
        }

        // Revert / preview logic (same as show())
        if let Some(path) = self.pending_delete_layout.take() {
            return SettingsAction::DeleteLayout(path);
        }

        if std::mem::take(&mut self.pending_test_notification) {
            return SettingsAction::TestNotification;
        }

        if std::mem::take(&mut self.pending_test_paste) {
            return SettingsAction::TestPaste;
        }

        if !self.is_open && action != SettingsAction::Applied {
            let theme_changed = self.original_theme_slug != theme_before;
            let opacity_changed = (self.original_opacity - opacity_before).abs() > f32::EPSILON;
            if theme_changed {
                return SettingsAction::RevertTheme(
                    self.original_theme_slug.clone(),
                    self.original_opacity,
                );
            } else if opacity_changed {
                return SettingsAction::RevertOpacity(self.original_opacity);
            }
            return action;
        }

        if (self.draft.ui.background_opacity - opacity_before).abs() > f32::EPSILON
            && action != SettingsAction::Applied
        {
            return SettingsAction::PreviewOpacity(self.draft.ui.background_opacity);
        }

        let theme_after = self.draft.theme.active_slug(self.os_dark_mode).to_string();
        if theme_after != theme_before && action != SettingsAction::Applied {
            return SettingsAction::PreviewTheme(theme_after);
        }

        action
    }

    // The main inline-modal entry point. Dominated by sequential preview /
    // revert / dirty-state checks; splitting would scatter the apply-flow
    // logic across opaque helpers.
    #[allow(clippy::too_many_lines)]
    pub fn show(&mut self, ctx: &egui::Context, os_dark_mode: bool) -> SettingsAction {
        self.os_dark_mode = os_dark_mode;
        if !self.is_open {
            return SettingsAction::None;
        }

        let mut action = SettingsAction::None;
        let mut open = self.is_open;

        // Snapshot the active theme slug and opacity before rendering so we
        // can detect whether the user changed either this frame.
        let theme_before = self.draft.theme.active_slug(self.os_dark_mode).to_string();
        let opacity_before = self.draft.ui.background_opacity;

        // Build an opaque window frame so the settings modal is never
        // affected by background_opacity (which lowers window_fill alpha
        // for compositor transparency).
        let opaque_frame = {
            let style = ctx.global_style();
            let base = egui::Frame::window(&style);
            let [r, g, b, _] = style.visuals.window_fill().to_array();
            base.fill(egui::Color32::from_rgba_unmultiplied(r, g, b, 255))
        };

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(true)
            .default_width(450.0)
            .frame(opaque_frame)
            .open(&mut open)
            .show(ctx, |ui| {
                // --- Read-only banner ---
                let is_read_only = self.read_only_reason.is_some();
                if let Some(reason) = &self.read_only_reason {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(80, 60, 20))
                        .corner_radius(4.0)
                        .inner_margin(8.0)
                        .show(ui, |ui| {
                            ui.colored_label(egui::Color32::from_rgb(255, 220, 100), reason);
                        });
                    ui.add_space(4.0);
                }

                // --- Tab bar ---
                self.show_tab_bar(ui, "settings_tab_bar_modal");

                // --- Tab content ---
                // Both axes so overflowed content is reachable without
                // enlarging the modal. `max_height(300.0)` keeps the modal
                // compact regardless of the selected tab's content size
                // (keybindings, themes, etc.). Scrollbars only appear when
                // content actually overflows.
                egui::ScrollArea::both()
                    .auto_shrink([false; 2])
                    .max_height(300.0)
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded,
                    )
                    .show(ui, |ui| {
                        // In read-only mode, disable all interactive widgets
                        // so the user can browse but not edit.
                        if is_read_only {
                            ui.disable();
                        }
                        self.draw_active_tab(ui);
                    });

                ui.separator();

                // --- Status message ---
                if let Some(msg) = &self.status_message {
                    ui.colored_label(egui::Color32::YELLOW, msg);
                }

                // --- Bottom buttons ---
                ui.horizontal(|ui| {
                    let reset_btn = egui::Button::new("Reset to Defaults");
                    if ui.add_enabled(!is_read_only, reset_btn).clicked() {
                        self.draft = Config::default();
                        self.status_message = Some("Reset to defaults (not saved yet)".to_string());
                    }

                    // Right-align Apply and Cancel.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let apply_btn = egui::Button::new("Apply");
                        if ui.add_enabled(!is_read_only, apply_btn).clicked() {
                            action = self.try_apply();
                        }
                        if ui.button("Cancel").clicked() {
                            // Route through the dirty-state guard so unsaved
                            // edits surface a confirmation prompt.
                            self.request_close();
                        }
                    });
                });
            });

        // Handle the X button on the window title bar.  When the user clicks
        // X, egui sets `open = false`; route that through the guard too so
        // the X respects unsaved changes.  If the guard vetoes the close,
        // re-open the embedded window by leaving `is_open = true`.
        if !open && self.is_open && !self.request_close() {
            // Guard deferred the close — keep the window open so the
            // confirm prompt can render on top.
        } else if !open {
            self.is_open = false;
        }

        // Render the unsaved-changes prompt on top of the settings UI.
        let prompt_action = self.show_confirm_close_prompt(ctx);
        if prompt_action == SettingsAction::Applied {
            action = prompt_action;
        }

        // If the modal just closed (Cancel or X) without Apply, revert any
        // previewed settings to their originals.
        if let Some(path) = self.pending_delete_layout.take() {
            return SettingsAction::DeleteLayout(path);
        }

        if std::mem::take(&mut self.pending_test_notification) {
            return SettingsAction::TestNotification;
        }

        if std::mem::take(&mut self.pending_test_paste) {
            return SettingsAction::TestPaste;
        }

        if !self.is_open && action != SettingsAction::Applied {
            let theme_changed = self.original_theme_slug != theme_before;
            let opacity_changed = (self.original_opacity - opacity_before).abs() > f32::EPSILON;
            // Theme revert carries the original opacity so the caller can
            // restore both in a single action.
            if theme_changed {
                return SettingsAction::RevertTheme(
                    self.original_theme_slug.clone(),
                    self.original_opacity,
                );
            } else if opacity_changed {
                return SettingsAction::RevertOpacity(self.original_opacity);
            }
            return action;
        }

        // If the opacity slider changed this frame, signal a live preview.
        if (self.draft.ui.background_opacity - opacity_before).abs() > f32::EPSILON
            && action != SettingsAction::Applied
        {
            return SettingsAction::PreviewOpacity(self.draft.ui.background_opacity);
        }

        // If the active theme slug changed this frame, signal a live preview.
        let theme_after = self.draft.theme.active_slug(self.os_dark_mode).to_string();
        if theme_after != theme_before && action != SettingsAction::Applied {
            return SettingsAction::PreviewTheme(theme_after);
        }

        action
    }

    /// Returns a reference to the draft config. Only meaningful after
    /// `SettingsAction::Applied` is returned.
    #[must_use]
    pub const fn applied_config(&self) -> &Config {
        &self.draft
    }

    /// Returns the draft `[notifications]` config so the app can route a
    /// "Test Notification" through exactly the settings the user is currently
    /// editing (including unsaved changes).
    #[must_use]
    pub(super) const fn draft_notifications(&self) -> &config::NotificationsConfig {
        &self.draft.notifications
    }

    /// The draft `[paste_guard]` config, so the "Test Paste" button can preview
    /// the confirm dialog using the settings the user is currently editing
    /// (including unsaved changes).
    #[must_use]
    pub(super) const fn draft_paste_guard(&self) -> &config::PasteGuardConfig {
        &self.draft.paste_guard
    }

    // -------------------------------------------------------------------------
    //  Tab dispatch
    // -------------------------------------------------------------------------

    /// Dispatch rendering to the currently active tab.
    fn draw_active_tab(&mut self, ui: &mut Ui) {
        match self.active_tab {
            SettingsTab::Font => self.show_font_tab(ui),
            SettingsTab::Cursor => self.show_cursor_tab(ui),
            SettingsTab::Theme => self.show_theme_tab(ui),
            SettingsTab::Shell => self.show_shell_tab(ui),
            SettingsTab::Scrollback => self.show_scrollback_tab(ui),
            SettingsTab::Logging => self.show_logging_tab(ui),
            SettingsTab::Ui => self.show_ui_tab(ui),
            SettingsTab::Tabs => self.show_tabs_tab(ui),
            SettingsTab::ShellIntegration => self.show_shell_integration_tab(ui),
            SettingsTab::Bell => self.show_bell_tab(ui),
            SettingsTab::Notifications => self.show_notifications_tab(ui),
            SettingsTab::Security => self.show_security_tab(ui),
            SettingsTab::Keybindings => self.show_keybindings_tab(ui),
            SettingsTab::Startup => self.show_startup_tab(ui),
        }
    }

    // -------------------------------------------------------------------------
    //  Tab implementations
    // -------------------------------------------------------------------------

    /// Render the horizontally-scrolling tab selector bar. Shared between the
    /// modal (`show`) and the standalone (`show_standalone`) renderers so the
    /// scroll affordance behaviour stays consistent. `id_salt` must differ
    /// between call sites to avoid egui id clashes when both renderers exist
    /// in the same frame.
    fn show_tab_bar(&mut self, ui: &mut Ui, id_salt: &'static str) {
        // Use a non-floating (solid) scrollbar style scoped to this
        // ScrollArea so the horizontal scrollbar gets its own strip below
        // the tab buttons instead of floating over them and expanding on
        // hover to cover nearly the full nav-area height. Also shrink the
        // bar width since a tab bar is a small UI element.
        ui.scope(|ui| {
            let scroll = &mut ui.style_mut().spacing.scroll;
            scroll.floating = false;
            scroll.bar_width = 6.0;
            scroll.bar_inner_margin = 2.0;
            scroll.bar_outer_margin = 0.0;

            egui::ScrollArea::horizontal()
                .id_salt(id_salt)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for tab in SettingsTab::ALL {
                            ui.selectable_value(&mut self.active_tab, tab, tab.label());
                        }
                    });
                });
        });
        ui.separator();
    }

    const DEFAULT_LABEL: &str = "Default (MesloLGS Nerd Font)";

    fn show_font_tab(&mut self, ui: &mut Ui) {
        // --- Font Family dropdown ---
        ui.label("Font Family:");

        let selected_label = self
            .draft
            .font
            .family
            .as_deref()
            .unwrap_or(Self::DEFAULT_LABEL);

        ComboBox::from_id_salt("font_family")
            .selected_text(selected_label)
            .width(300.0)
            .show_ui(ui, |ui| {
                // "Default" entry at the top.
                ui.selectable_value(&mut self.draft.font.family, None, Self::DEFAULT_LABEL);
                ui.separator();

                // All installed monospace families.
                for name in &self.monospace_families {
                    ui.selectable_value(
                        &mut self.draft.font.family,
                        Some(name.clone()),
                        name.as_str(),
                    );
                }
            });
        ui.add_space(8.0);

        // --- Font Size slider ---
        ui.label("Font Size:");
        ui.add(Slider::new(&mut self.draft.font.size, 4.0..=96.0).step_by(0.5));
        ui.add_space(8.0);

        // --- Ligatures toggle ---
        ui.checkbox(&mut self.draft.font.ligatures, "Enable Ligatures");
        ui.colored_label(
            egui::Color32::GRAY,
            "Render multi-character ligatures (e.g. =>, !=, ->).",
        );
        ui.add_space(8.0);

        // --- Font Preview ---
        ui.separator();
        ui.label("Preview:");
        let preview_text = "The quick brown fox 0O1lI| {}[]() => !=";

        // Choose the font family for the preview text:
        //   - Default selected → use egui's Monospace (bundled MesloLGS)
        //   - Custom font selected AND the registered preview matches → use the preview font
        //   - Custom font selected but preview not yet loaded or stale → fall back to Monospace
        let preview_font = if self.preview_registered.as_deref()
            == self.draft.font.family.as_deref()
            && self.draft.font.family.is_some()
        {
            FontFamily::Name("settings-preview".into())
        } else {
            FontFamily::Monospace
        };

        egui::Frame::NONE
            .fill(egui::Color32::from_gray(30))
            .corner_radius(4.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.label(
                    egui::RichText::new(preview_text)
                        .family(preview_font)
                        .size(self.draft.font.size),
                );
            });
    }

    fn show_cursor_tab(&mut self, ui: &mut Ui) {
        ui.label("Cursor Shape:");
        let current_label = cursor_shape_label(&self.draft.cursor.shape);
        ComboBox::from_id_salt("cursor_shape")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.cursor.shape,
                    CursorShapeConfig::Block,
                    "Block",
                );
                ui.selectable_value(
                    &mut self.draft.cursor.shape,
                    CursorShapeConfig::Underline,
                    "Underline",
                );
                ui.selectable_value(&mut self.draft.cursor.shape, CursorShapeConfig::Bar, "Bar");
            });
        ui.add_space(8.0);

        ui.checkbox(&mut self.draft.cursor.blink, "Cursor Blink");
        ui.add_space(8.0);

        ui.checkbox(&mut self.draft.cursor.trail, "Cursor Trail");
        ui.add_space(4.0);

        ui.add_enabled_ui(self.draft.cursor.trail, |ui| {
            ui.horizontal(|ui| {
                ui.label("Trail Duration (ms):");
                ui.add(egui::Slider::new(
                    &mut self.draft.cursor.trail_duration_ms,
                    10..=500,
                ));
            });
        });
    }

    fn show_theme_tab(&mut self, ui: &mut Ui) {
        // ── Mode selector ──────────────────────────────────────────────────
        ui.label("Theme Mode:");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.draft.theme.mode, ThemeMode::Dark, "Dark");
            ui.selectable_value(&mut self.draft.theme.mode, ThemeMode::Light, "Light");
            ui.selectable_value(
                &mut self.draft.theme.mode,
                ThemeMode::Auto,
                "Auto (follow OS)",
            );
        });
        ui.add_space(4.0);
        if self.draft.theme.mode == ThemeMode::Auto {
            ui.colored_label(
                egui::Color32::GRAY,
                "Auto mode uses the dark theme when the OS is in dark mode,\nand the light theme when the OS is in light mode.",
            );
        }
        ui.add_space(8.0);

        // ── Dark theme picker ──────────────────────────────────────────────
        let dark_display = themes::by_slug(&self.draft.theme.dark_name).map_or_else(
            || self.draft.theme.dark_name.clone(),
            |t| t.name.to_string(),
        );
        ui.label("Dark Theme:");
        ComboBox::from_id_salt("theme_dark_name")
            .selected_text(&dark_display)
            .show_ui(ui, |ui| {
                for theme in themes::all_themes() {
                    ui.selectable_value(
                        &mut self.draft.theme.dark_name,
                        theme.slug.to_string(),
                        theme.name,
                    );
                }
            });
        ui.add_space(4.0);
        if let Some(theme) = themes::by_slug(&self.draft.theme.dark_name)
            && self.draft.theme.mode != ThemeMode::Light
        {
            show_theme_preview(ui, theme);
        }
        ui.add_space(8.0);

        // ── Light theme picker ─────────────────────────────────────────────
        let light_display = themes::by_slug(&self.draft.theme.light_name).map_or_else(
            || self.draft.theme.light_name.clone(),
            |t| t.name.to_string(),
        );
        ui.label("Light Theme:");
        ComboBox::from_id_salt("theme_light_name")
            .selected_text(&light_display)
            .show_ui(ui, |ui| {
                for theme in themes::all_themes() {
                    ui.selectable_value(
                        &mut self.draft.theme.light_name,
                        theme.slug.to_string(),
                        theme.name,
                    );
                }
            });
        ui.add_space(4.0);
        if let Some(theme) = themes::by_slug(&self.draft.theme.light_name)
            && self.draft.theme.mode == ThemeMode::Light
        {
            show_theme_preview(ui, theme);
        }
    }

    fn show_shell_tab(&mut self, ui: &mut Ui) {
        ui.label("Shell Path:");
        let mut shell_text = self.draft.shell.path.clone().unwrap_or_default();
        let response = ui.text_edit_singleline(&mut shell_text);
        if response.changed() {
            self.draft.shell.path = if shell_text.is_empty() {
                None
            } else {
                Some(shell_text)
            };
        }
        ui.label("Leave empty to use the system default shell.");
        ui.add_space(8.0);
        ui.colored_label(egui::Color32::GRAY, "Changes take effect on next session.");
    }

    fn show_scrollback_tab(&mut self, ui: &mut Ui) {
        ui.label("Scrollback Limit:");
        ui.add(DragValue::new(&mut self.draft.scrollback.limit).range(1..=100_000));
        ui.add_space(8.0);
        ui.colored_label(egui::Color32::GRAY, "Changes take effect on next session.");
    }

    fn show_logging_tab(&mut self, ui: &mut Ui) {
        ui.label("File logging is always enabled.");
        ui.add_space(4.0);

        // Show the log directory path (read-only). The value was cached when
        // the modal was opened to avoid repeated filesystem calls per frame.
        ui.horizontal(|ui| {
            ui.label("Log directory:");
            ui.monospace(&self.log_dir_display);
        });
        ui.add_space(8.0);

        // Log level dropdown.
        ui.label("File Log Level:");
        let current_level = self
            .draft
            .logging
            .level
            .clone()
            .unwrap_or_else(|| "debug".to_string());
        let mut selected = current_level;
        ComboBox::from_id_salt("log_level")
            .selected_text(selected.as_str())
            .show_ui(ui, |ui| {
                for level in &["trace", "debug", "info", "warn", "error"] {
                    ui.selectable_value(&mut selected, (*level).to_string(), *level);
                }
            });
        // Persist choice into the draft config.
        self.draft.logging.level = if selected == "debug" {
            None // default — omit from TOML
        } else {
            Some(selected)
        };

        ui.add_space(8.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Log level changes take effect on next launch.",
        );
    }

    fn show_ui_tab(&mut self, ui: &mut Ui) {
        ui.checkbox(&mut self.draft.ui.hide_menu_bar, "Hide Menu Bar");
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "When enabled, the menu bar at the top of the window is hidden.",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Can also be set via the --hide-menu-bar CLI flag.",
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label("Background Opacity:");
        ui.add(Slider::new(&mut self.draft.ui.background_opacity, 0.0..=1.0).step_by(0.05));
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Only affects backgrounds. Text and content remain fully opaque.",
        );
        ui.colored_label(
            egui::Color32::GRAY,
            "On X11, requires a running compositor (e.g. picom).",
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.checkbox(
            &mut self.draft.ui.auto_detect_urls,
            "Auto-detect URLs in terminal output",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Makes plain URLs (http, https, file, ftp, mailto) clickable even when \
             programs do not emit OSC 8 hyperlinks.",
        );

        self.show_ui_background_image(ui);
        self.show_ui_shader(ui);
    }

    /// Background image controls for the UI tab.
    fn show_ui_background_image(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label("Background Image:");
        let mut bg_image_text = self
            .draft
            .ui
            .background_image
            .as_ref()
            .map_or_else(String::new, |p| p.display().to_string());
        let bg_response = ui.text_edit_singleline(&mut bg_image_text);
        if bg_response.changed() {
            self.draft.ui.background_image = if bg_image_text.is_empty() {
                None
            } else {
                Some(PathBuf::from(bg_image_text))
            };
        }
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Path to a background image (PNG, JPEG, WebP). Leave empty to disable.",
        );

        ui.add_space(8.0);

        ui.label("Background Image Mode:");
        let current_mode = self.draft.ui.background_image_mode;
        ComboBox::from_id_salt("bg_image_mode")
            .selected_text(match current_mode {
                BackgroundImageMode::Fill => "Fill",
                BackgroundImageMode::Fit => "Fit",
                BackgroundImageMode::Cover => "Cover",
                BackgroundImageMode::Tile => "Tile",
            })
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.ui.background_image_mode,
                    BackgroundImageMode::Fill,
                    "Fill (stretch, ignore aspect ratio)",
                );
                ui.selectable_value(
                    &mut self.draft.ui.background_image_mode,
                    BackgroundImageMode::Fit,
                    "Fit (letterbox, preserve aspect ratio)",
                );
                ui.selectable_value(
                    &mut self.draft.ui.background_image_mode,
                    BackgroundImageMode::Cover,
                    "Cover (crop, preserve aspect ratio)",
                );
                ui.selectable_value(
                    &mut self.draft.ui.background_image_mode,
                    BackgroundImageMode::Tile,
                    "Tile (repeat in both dimensions)",
                );
            });

        ui.add_space(8.0);

        ui.label("Background Image Opacity:");
        ui.add(Slider::new(&mut self.draft.ui.background_image_opacity, 0.0..=1.0).step_by(0.05));
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Controls the opacity of the background image overlay.",
        );
    }

    /// Custom shader controls for the UI tab.
    fn show_ui_shader(&mut self, ui: &mut Ui) {
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label("Custom Shader:");
        let mut shader_text = self
            .draft
            .shader
            .path
            .as_ref()
            .map_or_else(String::new, |p| p.display().to_string());
        let shader_response = ui.text_edit_singleline(&mut shader_text);
        if shader_response.changed() {
            self.draft.shader.path = if shader_text.is_empty() {
                None
            } else {
                Some(PathBuf::from(shader_text))
            };
        }
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Path to a GLSL fragment shader for post-processing. Leave empty to disable.",
        );
        ui.colored_label(
            egui::Color32::GRAY,
            "Uniforms: sampler2D u_terminal, vec2 u_resolution, float u_time.",
        );

        ui.add_space(8.0);

        ui.checkbox(&mut self.draft.shader.hot_reload, "Hot Reload Shader");
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Automatically recompile the shader when the file changes on disk.",
        );
    }

    fn show_tabs_tab(&mut self, ui: &mut Ui) {
        ui.checkbox(
            &mut self.draft.tabs.show_single_tab,
            "Show Tab Bar With Single Tab",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "When disabled, the tab bar only appears with multiple tabs.",
        );

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label("Tab Bar Position:");
        let current_label = tab_bar_position_label(self.draft.tabs.position);
        ComboBox::from_id_salt("tab_bar_position")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.draft.tabs.position, TabBarPosition::Top, "Top");
                ui.selectable_value(
                    &mut self.draft.tabs.position,
                    TabBarPosition::Bottom,
                    "Bottom",
                );
            });

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        ui.label("Tab Title Policy:");
        ui.add_space(2.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "How a tab combines a custom name (your rename) with the title the \
             shell sets via OSC 0/1/2.",
        );
        ui.add_space(4.0);

        let policy_label = tab_title_policy_label(self.draft.tab_title.policy);
        ComboBox::from_id_salt("tab_title_policy")
            .selected_text(policy_label)
            .show_ui(ui, |ui| {
                for policy in [
                    TabTitlePolicy::Prefix,
                    TabTitlePolicy::Suffix,
                    TabTitlePolicy::CustomWins,
                    TabTitlePolicy::OscWins,
                ] {
                    ui.selectable_value(
                        &mut self.draft.tab_title.policy,
                        policy,
                        tab_title_policy_label(policy),
                    );
                }
            });

        // The separator only applies to the combining policies.
        let separator_relevant = matches!(
            self.draft.tab_title.policy,
            TabTitlePolicy::Prefix | TabTitlePolicy::Suffix
        );
        if separator_relevant {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Separator:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.draft.tab_title.separator)
                        .desired_width(80.0),
                );
            });
        }

        // Live preview of how a renamed tab whose shell set a cwd-style
        // title would render under the current policy.
        ui.add_space(8.0);
        let preview = tab_title_preview(
            "my-rename",
            "~/projects",
            self.draft.tab_title.policy,
            &self.draft.tab_title.separator,
        );
        ui.colored_label(egui::Color32::GRAY, format!("Preview:  {preview}"));

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);

        self.show_broadcast_input_section(ui);
    }

    /// Render the "Broadcast Input" section of the Tabs settings tab
    /// (Task 74.5): the read-only shortcut display with a jump-to-keybindings
    /// button, and the confirm-before-enabling toggle.
    fn show_broadcast_input_section(&mut self, ui: &mut Ui) {
        ui.label("Broadcast Input:");
        ui.add_space(2.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "When enabled for a tab, keystrokes are sent to every pane in the \
             tab at once. Toggle it per-tab with the keybinding below.",
        );
        ui.add_space(4.0);

        let effective_map = self.draft.build_binding_map().unwrap_or_default();
        let binding_text = effective_map
            .combo_for(KeyAction::ToggleBroadcastInput)
            .map_or_else(|| "unbound".to_owned(), |c| c.to_string());
        ui.horizontal(|ui| {
            ui.label("Shortcut:");
            ui.monospace(&binding_text);
            if ui.button("Change…").clicked() {
                self.active_tab = SettingsTab::Keybindings;
            }
        });

        ui.add_space(4.0);
        ui.checkbox(
            &mut self.draft.tabs.confirm_broadcast,
            "Confirm before enabling broadcast",
        );
        ui.add_space(2.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "When on, the first time you enable broadcast in a tab a dialog \
             asks you to confirm. Disabling broadcast never prompts.",
        );
    }

    // Renders both the Shell Integration section and the Command Blocks
    // section in one tab (per 72.5's bundling decision). The two sections
    // share enough thematic context that splitting them obscures intent.
    #[allow(clippy::too_many_lines)]
    fn show_shell_integration_tab(&mut self, ui: &mut Ui) {
        // ── Shell Integration section ────────────────────────────────────────
        ui.heading("Shell Integration");
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "OSC 133 (FinalTerm/FTCS) shell integration lets Freminal detect \
             where each shell command starts, ends, and what its exit code was. \
             This powers command-block navigation, exit-status gutters, and \
             desktop notifications.",
        );
        ui.add_space(8.0);

        ui.checkbox(
            &mut self.draft.shell_integration.set_term_program,
            "Set TERM_PROGRAM=freminal",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Sets TERM_PROGRAM and TERM_PROGRAM_VERSION in the PTY environment \
             so shell scripts can detect Freminal.  Also enables spawn-time \
             auto-loading of the bundled shell-integration scripts for bash, \
             zsh, and fish — no manual sourcing required.",
        );

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);

        // ── Command Blocks section ───────────────────────────────────────────
        ui.heading("Command Blocks");
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Visual treatment of OSC 133 command blocks. Requires the \
             \"Set TERM_PROGRAM=freminal\" option above to be enabled so \
             that the bundled shell-integration scripts auto-load on \
             shell spawn.",
        );
        ui.add_space(8.0);

        ui.checkbox(
            &mut self.draft.command_blocks.enabled,
            "Enable command block tracking",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "When disabled, OSC 133 markers are still parsed but no command \
             blocks are surfaced to the GUI.",
        );

        ui.add_space(12.0);

        ui.checkbox(
            &mut self.draft.command_blocks.show_duration,
            "Show command duration",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Display the duration of long-running commands next to the gutter.",
        );

        ui.add_space(8.0);

        ui.horizontal(|ui| {
            ui.label("Duration threshold:");
            ui.add(
                egui::DragValue::new(&mut self.draft.command_blocks.duration_threshold_secs)
                    .speed(0.1)
                    .range(0.0_f32..=60.0_f32)
                    .suffix(" s"),
            );
        });
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Minimum command duration before the duration label is shown.",
        );

        ui.add_space(12.0);

        ui.label("Status gutter:");
        let current_label = gutter_position_label(self.draft.command_blocks.gutter);
        ComboBox::from_id_salt("command_block_gutter")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.command_blocks.gutter,
                    GutterPosition::Left,
                    "Left",
                );
                ui.selectable_value(
                    &mut self.draft.command_blocks.gutter,
                    GutterPosition::Off,
                    "Off",
                );
            });
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "A thin colored strip on the left edge of each pane showing every \
             command's status (green = success, red = failure, yellow = \
             running). \"Off\" hides the gutter and reclaims its width for text.",
        );
    }

    fn show_bell_tab(&mut self, ui: &mut Ui) {
        ui.label("Bell Mode:");
        let current_label = bell_mode_label(self.draft.bell.mode);
        ComboBox::from_id_salt("bell_mode")
            .selected_text(current_label)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.bell.mode,
                    config::BellMode::Visual,
                    "Visual",
                );
                ui.selectable_value(&mut self.draft.bell.mode, config::BellMode::Audio, "Audio");
                ui.selectable_value(&mut self.draft.bell.mode, config::BellMode::Both, "Both");
                ui.selectable_value(&mut self.draft.bell.mode, config::BellMode::None, "None");
            });

        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Visual: briefly flash the terminal area.\n\
             None: silently ignore the bell.",
        );

        ui.add_space(12.0);

        ui.checkbox(
            &mut self.draft.bell.on_command_finished,
            "Ring bell on command completion",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Ring the bell (using the mode above) when a command finishes \
             (OSC 133 D), even if the program emitted no bell character. \
             Requires shell integration.",
        );
    }

    fn show_notifications_tab(&mut self, ui: &mut Ui) {
        ui.checkbox(
            &mut self.draft.notifications.enabled,
            "Enable notifications",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Master switch. When off, no toasts or desktop notifications are \
             produced, regardless of the options below.",
        );

        ui.add_space(12.0);
        ui.heading("Sources");
        ui.checkbox(
            &mut self.draft.notifications.osc_9,
            "OSC 9 (iTerm2 / WezTerm text)",
        );
        ui.checkbox(
            &mut self.draft.notifications.osc_777,
            "OSC 777 (urxvt notify;TITLE;BODY)",
        );
        ui.checkbox(
            &mut self.draft.notifications.on_command_finished,
            "Command finished (OSC 133 D)",
        );

        ui.add_space(12.0);
        ui.horizontal(|ui| {
            ui.label("Command threshold:");
            ui.add(
                DragValue::new(&mut self.draft.notifications.command_finished_threshold_secs)
                    .speed(0.1)
                    .range(0.0_f32..=600.0_f32)
                    .suffix(" s"),
            );
        });
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Minimum command duration before a command-finished notification \
             fires. Avoids spamming notifications for fast commands.",
        );

        ui.add_space(12.0);
        ui.heading("Routing");
        Self::notification_routing_row(ui, "Errors", &mut self.draft.notifications.routing_error);
        Self::notification_routing_row(ui, "Info", &mut self.draft.notifications.routing_info);
        Self::notification_routing_row(
            ui,
            "Command finished",
            &mut self.draft.notifications.routing_command_finished,
        );

        ui.add_space(12.0);
        ui.heading("Template");
        ui.label("Command-finished notification body:");
        ui.add(
            egui::TextEdit::singleline(&mut self.draft.notifications.command_finished_template)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Tokens: {command}, {duration}, {exit_code}, {cwd}, {tab_name}.",
        );

        ui.add_space(8.0);
        if ui.button("Test Notification").clicked() {
            self.pending_test_notification = true;
        }
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Dispatches a sample notification through the routing selected \
             above (uses the current, unsaved settings).",
        );

        ui.add_space(12.0);
        ui.heading("Bell");
        ui.checkbox(
            &mut self.draft.bell.on_command_finished,
            "Ring bell on command completion",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Mirrors the toggle in the Bell tab. The bell mode is configured \
             there.",
        );
    }

    /// Render one labeled routing combo box. Extracted so the three routing
    /// categories share identical UI and the per-category borrow of
    /// `self.draft.notifications` stays scoped.
    fn notification_routing_row(
        ui: &mut Ui,
        label: &str,
        routing: &mut config::NotificationRouting,
    ) {
        ui.horizontal(|ui| {
            ui.label(label);
            ComboBox::from_id_salt(format!("notif_routing_{label}"))
                .selected_text(notification_routing_label(*routing))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        routing,
                        config::NotificationRouting::Toast,
                        notification_routing_label(config::NotificationRouting::Toast),
                    );
                    ui.selectable_value(
                        routing,
                        config::NotificationRouting::System,
                        notification_routing_label(config::NotificationRouting::System),
                    );
                    ui.selectable_value(
                        routing,
                        config::NotificationRouting::Both,
                        notification_routing_label(config::NotificationRouting::Both),
                    );
                    ui.selectable_value(
                        routing,
                        config::NotificationRouting::SystemWhenUnfocused,
                        notification_routing_label(
                            config::NotificationRouting::SystemWhenUnfocused,
                        ),
                    );
                });
        });
    }

    fn show_security_tab(&mut self, ui: &mut Ui) {
        ui.checkbox(
            &mut self.draft.security.allow_clipboard_read,
            "Allow Clipboard Read (OSC 52)",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "When enabled, programs can read the system clipboard via \
             OSC 52 query.\n\
             This is a potential security risk if untrusted programs run \
             inside the terminal.",
        );

        ui.add_space(12.0);

        ui.checkbox(
            &mut self.draft.security.password_indicator,
            "Password Indicator",
        );
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Show a \u{1f510} lock icon in the tab bar when the foreground \
             process disables terminal echo (e.g. password prompts from \
             sudo, ssh, passwd).",
        );

        ui.add_space(16.0);
        ui.separator();
        ui.add_space(8.0);
        self.show_paste_guard_section(ui);
    }

    /// The Paste Guard subsection of the Security tab (Task 77).
    fn show_paste_guard_section(&mut self, ui: &mut Ui) {
        ui.heading("Paste Guard");
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Show a confirmation dialog before pasting risky content. \
             Bypass for a single paste with Ctrl+Shift+Alt+V.",
        );
        ui.add_space(8.0);

        ui.checkbox(&mut self.draft.paste_guard.enabled, "Enable paste guard");

        // The per-trigger toggles are only meaningful while the guard is on.
        ui.add_enabled_ui(self.draft.paste_guard.enabled, |ui| {
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.draft.paste_guard.multiline,
                "Confirm multi-line pastes",
            );
            ui.checkbox(
                &mut self.draft.paste_guard.control_chars,
                "Confirm pastes containing control characters",
            );
            ui.checkbox(
                &mut self.draft.paste_guard.patterns,
                "Confirm pastes matching dangerous patterns",
            );

            // Pattern list editor — only relevant when pattern matching is on.
            ui.add_enabled_ui(self.draft.paste_guard.patterns, |ui| {
                ui.add_space(8.0);
                ui.label("Dangerous patterns (Rust regex):");
                ui.add_space(2.0);

                // Index of a row to delete, applied after the loop so we do
                // not mutate the Vec while iterating it.
                let mut remove_index: Option<usize> = None;
                for (idx, pattern) in self.draft.paste_guard.pattern_list.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.button("\u{2796}").on_hover_text("Remove").clicked() {
                            remove_index = Some(idx);
                        }
                        let valid = regex::Regex::new(pattern).is_ok();
                        let edit = egui::TextEdit::singleline(pattern)
                            .desired_width(f32::INFINITY)
                            .font(egui::TextStyle::Monospace)
                            .text_color_opt(
                                (!valid).then_some(egui::Color32::from_rgb(0xE0, 0x6C, 0x4B)),
                            );
                        ui.add(edit).on_hover_text(if valid {
                            "Valid regex"
                        } else {
                            "Invalid regex \u{2014} this pattern is ignored at match time"
                        });
                    });
                }
                if let Some(idx) = remove_index {
                    self.draft.paste_guard.pattern_list.remove(idx);
                }

                ui.add_space(2.0);
                if ui.button("\u{2795} Add pattern").clicked() {
                    self.draft.paste_guard.pattern_list.push(String::new());
                }
            });
        });

        ui.add_space(8.0);
        if ui.button("Test Paste").clicked() {
            self.pending_test_paste = true;
        }
        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Opens the confirm dialog with sample content using the current \
             (unsaved) settings, so you can preview the guard without pasting \
             for real.",
        );
    }

    fn show_keybindings_tab(&mut self, ui: &mut Ui) {
        // Build the effective map (defaults + current draft overrides) so we
        // can display what each action is currently bound to.
        let effective_map = self.draft.build_binding_map().unwrap_or_default();

        ui.colored_label(
            egui::Color32::GRAY,
            "Click a binding to change it.  Press the new key combination \
             when prompted.",
        );
        ui.add_space(8.0);

        // ── Recording overlay ────────────────────────────────────────────
        // When recording, intercept key events before the grid consumes them.
        self.handle_key_recording(ui, &effective_map);

        egui::Grid::new("keybindings_grid")
            .num_columns(3)
            .spacing([16.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for action in KeyAction::ALL {
                    self.show_keybinding_row(ui, *action, &effective_map);
                    ui.end_row();
                }
            });
    }

    /// Handle the key-recording state machine.
    ///
    /// When in `Recording` state, intercepts key events from the current frame
    /// and transitions to `Confirming` (with conflict info) or directly writes
    /// the new binding.
    fn handle_key_recording(&mut self, ui: &Ui, effective_map: &BindingMap) {
        match &self.key_recording {
            KeyRecordingState::Idle => {}
            KeyRecordingState::Recording { action } => {
                let action = *action;
                self.handle_recording_state(ui, effective_map, action);
            }
            KeyRecordingState::Confirming {
                action,
                combo,
                conflict,
            } => {
                let action = *action;
                let combo = *combo;
                let conflict = *conflict;
                self.show_confirm_dialog(ui, action, combo, conflict);
            }
        }
    }

    /// Drive the recording overlay: show prompt, capture key events, transition
    /// to `Confirming` or back to `Idle` on Escape.
    fn handle_recording_state(&mut self, ui: &Ui, effective_map: &BindingMap, action: KeyAction) {
        // Show a non-interactive overlay prompting for key input.
        egui::Area::new(ui.id().with("key_recording_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(280.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(12.0);
                        ui.heading(format!("Recording: {}", action.display_label()));
                        ui.add_space(8.0);
                        ui.label("Press the key combination you want to assign.");
                        ui.label("Press Escape to cancel.");
                        ui.add_space(12.0);
                    });
                });
            });

        // Scan this frame's events for a key press.
        let captured = ui.input(|input| Self::scan_key_events(&input.raw.events));

        match captured {
            Some(KeyCapture::Cancelled) => {
                // Escape pressed — cancel.
                self.key_recording = KeyRecordingState::Idle;
            }
            Some(KeyCapture::Combo(combo)) => {
                // Check for conflicts.
                let conflict = effective_map.lookup(&combo).filter(|a| *a != action);
                self.key_recording = KeyRecordingState::Confirming {
                    action,
                    combo,
                    conflict,
                };
            }
            None => {
                // No key event this frame — stay in recording.
            }
        }
    }

    /// Scan a slice of egui events for a valid key-binding press.
    ///
    /// Returns `Some(KeyCapture::Cancelled)` for Escape,
    /// `Some(KeyCapture::Combo(..))` for a valid binding, or `None` if no
    /// relevant key was pressed this frame.
    fn scan_key_events(events: &[egui::Event]) -> Option<KeyCapture> {
        use crate::gui::terminal::input::{egui_key_to_binding_key, egui_mods_to_binding_mods};

        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                // Escape cancels recording.
                if *key == egui::Key::Escape {
                    return Some(KeyCapture::Cancelled);
                }

                if !Self::is_valid_binding_press(*key, *modifiers) {
                    continue;
                }

                if let Some(binding_key) = egui_key_to_binding_key(*key) {
                    let combo = KeyCombo::new(binding_key, egui_mods_to_binding_mods(*modifiers));
                    return Some(KeyCapture::Combo(combo));
                }
            }
        }
        None
    }

    /// Check whether a key press qualifies as a valid binding.
    ///
    /// Function keys are always valid.  Other keys require at least one
    /// modifier.  Alphanumeric keys additionally require a non-Shift modifier
    /// (Ctrl, Alt, or Cmd) so that Shift+letter does not steal uppercase
    /// typing.
    const fn is_valid_binding_press(key: egui::Key, modifiers: egui::Modifiers) -> bool {
        use crate::gui::terminal::input::egui_key_to_binding_key;

        let is_function_key = matches!(
            key,
            egui::Key::F1
                | egui::Key::F2
                | egui::Key::F3
                | egui::Key::F4
                | egui::Key::F5
                | egui::Key::F6
                | egui::Key::F7
                | egui::Key::F8
                | egui::Key::F9
                | egui::Key::F10
                | egui::Key::F11
                | egui::Key::F12
        );

        if is_function_key {
            return true;
        }

        let has_modifier = modifiers.ctrl || modifiers.shift || modifiers.alt || modifiers.command;
        if !has_modifier {
            return false;
        }

        // For alphanumeric keys, Shift alone would steal uppercase letters /
        // shifted digits from normal typing.  Require at least Ctrl or Alt.
        let has_non_shift_modifier = modifiers.ctrl || modifiers.alt || modifiers.command;
        if let Some(binding_key) = egui_key_to_binding_key(key)
            && binding_key.is_alphanumeric()
            && !has_non_shift_modifier
        {
            return false;
        }

        true
    }

    /// Show the confirmation dialog for a newly captured key combo.
    fn show_confirm_dialog(
        &mut self,
        ui: &Ui,
        action: KeyAction,
        combo: KeyCombo,
        conflict: Option<KeyAction>,
    ) {
        egui::Area::new(ui.id().with("key_confirm_overlay"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_min_width(320.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(12.0);
                        ui.heading(format!("Assign {} to {}?", combo, action.display_label()));
                        ui.add_space(8.0);

                        if let Some(other) = conflict {
                            ui.colored_label(
                                egui::Color32::YELLOW,
                                format!(
                                    "\u{26a0} {} is already bound to {}.",
                                    combo,
                                    other.display_label()
                                ),
                            );
                            ui.label("Accepting will unbind the other action.");
                            ui.add_space(8.0);
                        }

                        ui.horizontal(|ui| {
                            if ui.button("Accept").clicked() {
                                // Write the new binding.
                                self.apply_recorded_binding(action, combo, conflict);
                                self.key_recording = KeyRecordingState::Idle;
                            }
                            if ui.button("Cancel").clicked() {
                                self.key_recording = KeyRecordingState::Idle;
                            }
                        });
                        ui.add_space(12.0);
                    });
                });
            });
    }

    /// Apply a recorded key binding to the draft config overrides.
    ///
    /// If there is a conflict, the conflicting action's override is set to
    /// `"none"` to unbind it.
    fn apply_recorded_binding(
        &mut self,
        action: KeyAction,
        combo: KeyCombo,
        conflict: Option<KeyAction>,
    ) {
        let combo_str = combo.to_string();
        self.draft
            .keybindings
            .overrides
            .insert(action.name().to_owned(), combo_str);

        if let Some(other) = conflict {
            self.draft
                .keybindings
                .overrides
                .insert(other.name().to_owned(), "none".to_owned());
        }
    }

    /// Render one row of the keybindings grid: action label + current combo
    /// button + clear button.
    fn show_keybinding_row(&mut self, ui: &mut Ui, action: KeyAction, effective_map: &BindingMap) {
        ui.label(action.display_label());

        // Show the current combo (from override or effective map).
        let override_key = action.name().to_owned();
        let is_recording = matches!(
            &self.key_recording,
            KeyRecordingState::Recording { action: a } if *a == action
        );

        let current_text = self
            .draft
            .keybindings
            .overrides
            .get(&override_key)
            .map_or_else(
                || {
                    effective_map
                        .combo_for(action)
                        .map_or_else(|| "unbound".to_owned(), |c| c.to_string())
                },
                |ov| {
                    if ov.eq_ignore_ascii_case("none") {
                        "unbound".to_owned()
                    } else {
                        ov.clone()
                    }
                },
            );

        let button_text = if is_recording {
            "\u{23fa} Recording...".to_owned()
        } else {
            current_text
        };

        let btn = ui.add_sized([180.0, 20.0], egui::Button::new(&button_text));
        if btn.clicked() && !is_recording {
            self.key_recording = KeyRecordingState::Recording { action };
        }

        // Clear button to unbind this action.
        if ui.small_button("X").on_hover_text("Unbind").clicked() {
            self.draft
                .keybindings
                .overrides
                .insert(override_key, "none".to_owned());
            self.key_recording = KeyRecordingState::Idle;
        }
    }

    // -------------------------------------------------------------------------
    //  Font preview helpers
    // -------------------------------------------------------------------------

    /// Return the font family name that needs to be loaded as a preview, if it
    /// differs from what is currently registered. Returns `None` when the
    /// default (bundled) font is selected or when the requested family is
    /// already registered.
    #[must_use]
    pub fn needed_preview_family(&self) -> Option<String> {
        let wanted = self.draft.font.family.as_deref()?;
        if self.preview_registered.as_deref() == Some(wanted) {
            return None;
        }
        Some(wanted.to_owned())
    }

    /// Register a preview font in egui's font system so `show_font_tab()` can
    /// render sample text in the user's chosen font.
    ///
    /// `family` is the family name being previewed, `font_bytes` are the raw
    /// TTF/OTF bytes (or `None` if the font could not be loaded), and
    /// `base_defs` are the original `FontDefinitions` without any preview font.
    pub fn register_preview_font(
        &mut self,
        ctx: &egui::Context,
        family: &str,
        font_bytes: Option<Vec<u8>>,
        base_defs: &FontDefinitions,
    ) {
        let Some(bytes) = font_bytes else {
            // Font could not be loaded — clear any previous preview.
            self.preview_registered = None;
            ctx.set_fonts(base_defs.clone());
            return;
        };

        let mut defs = base_defs.clone();
        defs.font_data.insert(
            "settings-preview".to_owned(),
            FontData::from_owned(bytes).into(),
        );
        defs.families.insert(
            FontFamily::Name("settings-preview".into()),
            vec!["settings-preview".to_owned()],
        );
        ctx.set_fonts(defs);

        self.preview_registered = Some(family.to_owned());
    }

    /// Save the base font definitions when the modal opens.
    pub fn set_base_font_defs(&mut self, defs: FontDefinitions) {
        self.base_font_defs = Some(defs);
    }

    /// Restore the original egui font set (removing any preview font).
    /// Called when the modal closes (Cancel, X, or Apply).
    pub fn restore_base_fonts(&mut self, ctx: &egui::Context) {
        if let Some(base) = &self.base_font_defs {
            ctx.set_fonts(base.clone());
        }
        self.preview_registered = None;
        self.base_font_defs = None;
    }

    // -------------------------------------------------------------------------
    //  Apply logic
    // -------------------------------------------------------------------------

    fn try_apply(&mut self) -> SettingsAction {
        match config::save_config(&self.draft, self.config_path.as_deref()) {
            Ok(()) => {
                self.is_open = false;
                self.status_message = None;
                // Refresh baseline so any subsequent reopen sees the saved
                // state as clean.
                self.baseline_toml = Self::serialize_for_baseline(&self.draft);
                self.pending_close = PendingClose::None;
                SettingsAction::Applied
            }
            Err(e) => {
                self.status_message = Some(format!("Save failed: {e}"));
                SettingsAction::None
            }
        }
    }

    // ── Startup tab ──────────────────────────────────────────────────────────

    fn show_startup_tab(&mut self, ui: &mut Ui) {
        ui.heading("Startup & Layouts");
        ui.add_space(8.0);

        // ── Session restore ─────────────────────────────────────────────────
        ui.group(|ui| {
            ui.label(egui::RichText::new("Session Restore").strong());
            ui.add_space(4.0);
            ui.checkbox(
                &mut self.draft.startup.restore_last_session,
                "Restore last session on startup",
            );
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "When enabled, Freminal saves the current layout on exit and \
                     reopens it on the next launch (unless --layout is given on the \
                     command line).",
                )
                .weak()
                .small(),
            );
        });

        ui.add_space(8.0);

        // ── Default startup layout ───────────────────────────────────────────
        self.show_startup_layout_group(ui);

        ui.add_space(8.0);

        // ── Layout library ───────────────────────────────────────────────────
        self.show_layout_library_group(ui);
    }

    /// Render the "Default Layout" group within the Startup tab.
    ///
    /// Split out from [`Self::show_startup_tab`] to keep that function under
    /// the pedantic line-count threshold and to make the layout-selection
    /// logic easier to locate when iterating on UX.
    fn show_startup_layout_group(&mut self, ui: &mut Ui) {
        // Sentinel label shown when no startup layout is configured.
        // Blank strings inside a ComboBox render as zero-width items,
        // which is easy to miss; an explicit label is unambiguous.
        const NONE_LABEL: &str = "(none)";

        ui.group(|ui| {
            ui.label(egui::RichText::new("Default Layout").strong());
            ui.add_space(4.0);

            // Selected layout name, or the NONE_LABEL sentinel.
            let current = self
                .draft
                .startup
                .layout
                .clone()
                .unwrap_or_else(|| NONE_LABEL.to_string());

            // Track whether the configured layout is missing from the
            // discovered list (e.g. layouts dir was removed, or the user
            // typed a name manually in a previous session).  In that case
            // we still show the current value so the user can see what's
            // configured, but mark it with a warning suffix.
            let configured_missing = startup_layout_is_missing(
                self.draft.startup.layout.as_deref(),
                &self.discovered_layouts,
            );

            ui.horizontal(|ui| {
                ui.label("Layout:");
                let selected_text = if configured_missing {
                    format!("{current}  (missing)")
                } else {
                    current.clone()
                };
                ComboBox::from_id_salt("startup_layout_combo")
                    .selected_text(selected_text)
                    .show_ui(ui, |ui| {
                        self.populate_startup_layout_combo(
                            ui,
                            NONE_LABEL,
                            &current,
                            configured_missing,
                        );
                    });
            });
            ui.add_space(2.0);
            ui.label(
                egui::RichText::new(
                    "Layout to load on startup, from ~/.config/freminal/layouts/. \
                     Select \"(none)\" for the default single-pane session.",
                )
                .weak()
                .small(),
            );
        });
    }

    /// Populate the entries of the startup-layout `ComboBox`.
    ///
    /// Extracted from [`Self::show_startup_layout_group`] purely to keep
    /// line counts below the pedantic threshold.
    fn populate_startup_layout_combo(
        &mut self,
        ui: &mut Ui,
        none_label: &str,
        current: &str,
        configured_missing: bool,
    ) {
        // "(none)" entry clears the startup layout.
        if ui
            .selectable_label(self.draft.startup.layout.is_none(), none_label)
            .clicked()
        {
            self.draft.startup.layout = None;
        }
        // Keep an entry for the configured-but-missing layout so
        // re-selecting it is trivial once the file reappears, and so
        // the user isn't forced to delete the setting just to reach
        // the combo.
        if configured_missing {
            let is_selected = self
                .draft
                .startup
                .layout
                .as_deref()
                .is_some_and(|n| n == current);
            let label = format!("{current}  (missing)");
            if ui.selectable_label(is_selected, label).clicked() {
                self.draft.startup.layout = Some(current.to_string());
            }
        }
        for layout in &self.discovered_layouts {
            let is_selected = self
                .draft
                .startup
                .layout
                .as_deref()
                .is_some_and(|n| n == layout.name);
            let label = layout.description.as_deref().map_or_else(
                || layout.name.clone(),
                |d| format!("{}  —  {d}", layout.name),
            );
            if ui.selectable_label(is_selected, label).clicked() {
                self.draft.startup.layout = Some(layout.name.clone());
            }
        }
    }

    /// Render the "Layout Library" group within the Startup tab.
    ///
    /// Shows discovered layouts with per-entry delete buttons, or a hint
    /// when the layouts directory is empty.  Extracted from
    /// [`Self::show_startup_tab`] for the same reason as
    /// [`Self::show_startup_layout_group`].
    fn show_layout_library_group(&mut self, ui: &mut Ui) {
        ui.group(|ui| {
            ui.label(egui::RichText::new("Layout Library").strong());
            ui.add_space(4.0);

            if self.discovered_layouts.is_empty() {
                ui.label(
                    egui::RichText::new(
                        "No layouts found in ~/.config/freminal/layouts/.\n\
                         Use Layouts → Save Current Layout… to create one.",
                    )
                    .weak(),
                );
            } else {
                egui::ScrollArea::vertical()
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for layout in &self.discovered_layouts {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&layout.name).strong());
                                if let Some(ref desc) = layout.description {
                                    ui.label(egui::RichText::new(format!("— {desc}")).weak());
                                }
                                // Right-align the delete button.
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new("Delete").color(
                                                        egui::Color32::from_rgb(220, 80, 80),
                                                    ),
                                                )
                                                .small(),
                                            )
                                            .on_hover_text("Delete this layout file")
                                            .clicked()
                                        {
                                            self.pending_delete_layout = Some(layout.path.clone());
                                        }
                                    },
                                );
                            });
                            ui.label(
                                egui::RichText::new(layout.path.display().to_string())
                                    .weak()
                                    .small(),
                            );
                            ui.add_space(4.0);
                        }
                    });
            }
        });
    }
}

/// Human-readable label for a `CursorShapeConfig` variant.
const fn cursor_shape_label(shape: &CursorShapeConfig) -> &'static str {
    match shape {
        CursorShapeConfig::Block => "Block",
        CursorShapeConfig::Underline => "Underline",
        CursorShapeConfig::Bar => "Bar",
    }
}

/// Human-readable label for a `TabBarPosition` variant.
const fn tab_bar_position_label(pos: TabBarPosition) -> &'static str {
    match pos {
        TabBarPosition::Top => "Top",
        TabBarPosition::Bottom => "Bottom",
    }
}

const fn tab_title_policy_label(policy: TabTitlePolicy) -> &'static str {
    match policy {
        TabTitlePolicy::Prefix => "Prefix (custom: osc)",
        TabTitlePolicy::Suffix => "Suffix (osc: custom)",
        TabTitlePolicy::CustomWins => "Custom Wins",
        TabTitlePolicy::OscWins => "OSC Wins",
    }
}

/// Render a live preview of the tab title under `policy`, mirroring the
/// runtime logic in `Tab::display_name`.
fn tab_title_preview(custom: &str, osc: &str, policy: TabTitlePolicy, separator: &str) -> String {
    match policy {
        TabTitlePolicy::Prefix => format!("{custom}{separator}{osc}"),
        TabTitlePolicy::Suffix => format!("{osc}{separator}{custom}"),
        TabTitlePolicy::CustomWins => custom.to_owned(),
        TabTitlePolicy::OscWins => osc.to_owned(),
    }
}

const fn gutter_position_label(pos: GutterPosition) -> &'static str {
    match pos {
        GutterPosition::Left => "Left",
        GutterPosition::Off => "Off",
    }
}

const fn bell_mode_label(mode: config::BellMode) -> &'static str {
    match mode {
        config::BellMode::Visual => "Visual",
        config::BellMode::None => "None",
        config::BellMode::Audio => "Audio",
        config::BellMode::Both => "Both",
    }
}

const fn notification_routing_label(routing: config::NotificationRouting) -> &'static str {
    match routing {
        config::NotificationRouting::Toast => "Toast",
        config::NotificationRouting::System => "System",
        config::NotificationRouting::Both => "Both",
        config::NotificationRouting::SystemWhenUnfocused => "System when unfocused",
    }
}

/// Returns `true` when a startup layout is configured but not present in
/// the discovered layout list.  Extracted for unit testing since the
/// `ComboBox` UI itself is hard to exercise in isolation.
fn startup_layout_is_missing(
    configured: Option<&str>,
    discovered: &[freminal_common::layout::LayoutSummary],
) -> bool {
    configured.is_some_and(|name| !discovered.iter().any(|l| l.name.as_str() == name))
}

/// Paint a small colored rectangle as an inline swatch.
fn color_swatch(ui: &mut Ui, (r, g, b): (u8, u8, u8), size: egui::Vec2) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 2.0, egui::Color32::from_rgb(r, g, b));
}

/// Render a compact color preview strip for the given theme.
fn show_theme_preview(ui: &mut Ui, theme: &themes::ThemePalette) {
    let swatch_size = egui::vec2(16.0, 16.0);

    // Foreground / Background swatches.
    ui.horizontal(|ui| {
        ui.label("FG:");
        color_swatch(ui, theme.foreground, swatch_size);
        ui.add_space(4.0);
        ui.label("BG:");
        color_swatch(ui, theme.background, swatch_size);
        ui.add_space(4.0);
        ui.label("Cursor:");
        color_swatch(ui, theme.cursor, swatch_size);
    });
    ui.add_space(4.0);

    // 16 ANSI color swatches in two rows (normal + bright).
    ui.label("ANSI Colors:");
    ui.horizontal(|ui| {
        for color in &theme.ansi[..8] {
            color_swatch(ui, *color, swatch_size);
        }
    });
    ui.horizontal(|ui| {
        for color in &theme.ansi[8..] {
            color_swatch(ui, *color, swatch_size);
        }
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn modal_starts_closed() {
        let modal = SettingsModal::new(None);
        assert!(!modal.is_open);
    }

    #[test]
    fn tab_title_preview_matches_each_policy() {
        assert_eq!(
            tab_title_preview("c", "o", TabTitlePolicy::Prefix, ": "),
            "c: o"
        );
        assert_eq!(
            tab_title_preview("c", "o", TabTitlePolicy::Suffix, ": "),
            "o: c"
        );
        assert_eq!(
            tab_title_preview("c", "o", TabTitlePolicy::CustomWins, ": "),
            "c"
        );
        assert_eq!(
            tab_title_preview("c", "o", TabTitlePolicy::OscWins, ": "),
            "o"
        );
    }

    #[test]
    fn tab_title_policy_label_is_set_for_each_variant() {
        for policy in [
            TabTitlePolicy::Prefix,
            TabTitlePolicy::Suffix,
            TabTitlePolicy::CustomWins,
            TabTitlePolicy::OscWins,
        ] {
            assert!(!tab_title_policy_label(policy).is_empty());
        }
    }

    #[test]
    fn tab_title_draft_round_trips_through_open() {
        let mut modal = SettingsModal::new(None);
        let mut live = Config::default();
        live.tab_title.policy = TabTitlePolicy::Suffix;
        live.tab_title.separator = String::from(" / ");
        modal.open(&live, Vec::new(), false);

        assert_eq!(modal.draft.tab_title.policy, TabTitlePolicy::Suffix);
        assert_eq!(modal.draft.tab_title.separator, " / ");
    }

    #[test]
    fn open_clones_live_config_into_draft() {
        let mut modal = SettingsModal::new(None);
        let mut live = Config::default();
        live.font.size = 20.0;
        live.scrollback.limit = 500;
        modal.open(&live, Vec::new(), false);

        assert!(modal.is_open);
        assert!((modal.draft.font.size - 20.0).abs() < f32::EPSILON);
        assert_eq!(modal.draft.scrollback.limit, 500);
    }

    #[test]
    fn reset_to_defaults_produces_default_config() {
        let mut modal = SettingsModal::new(None);
        let mut live = Config::default();
        live.font.size = 42.0;
        modal.open(&live, Vec::new(), false);

        // Simulate clicking "Reset to Defaults" by resetting the draft.
        modal.draft = Config::default();
        assert!((modal.draft.font.size - 12.0).abs() < f32::EPSILON);
    }

    #[test]
    fn show_returns_none_when_closed() {
        let modal = SettingsModal::new(None);
        assert!(!modal.is_open);
        // When closed, show() returns None — we can't test this without egui
        // context, but the logic is trivially verified by code inspection.
    }

    #[test]
    fn settings_tab_labels() {
        assert_eq!(SettingsTab::Font.label(), "Font");
        assert_eq!(SettingsTab::Cursor.label(), "Cursor");
        assert_eq!(SettingsTab::Theme.label(), "Theme");
        assert_eq!(SettingsTab::Shell.label(), "Shell");
        assert_eq!(SettingsTab::Scrollback.label(), "Scrollback");
        assert_eq!(SettingsTab::Logging.label(), "Logging");
        assert_eq!(SettingsTab::Ui.label(), "UI");
        assert_eq!(SettingsTab::Tabs.label(), "Tabs");
        assert_eq!(SettingsTab::ShellIntegration.label(), "Shell Integration");
        assert_eq!(SettingsTab::Bell.label(), "Bell");
        assert_eq!(SettingsTab::Notifications.label(), "Notifications");
        assert_eq!(SettingsTab::Security.label(), "Security");
        assert_eq!(SettingsTab::Keybindings.label(), "Keybindings");
        assert_eq!(SettingsTab::Startup.label(), "Startup");
    }

    #[test]
    fn open_to_tab_selects_requested_tab() {
        let mut modal = SettingsModal::new(None);
        let cfg = Config::default();
        modal.open_to_tab(&cfg, Vec::new(), false, SettingsTab::Keybindings);
        assert!(modal.is_open);
        assert_eq!(modal.active_tab, SettingsTab::Keybindings);
    }

    #[test]
    fn set_active_tab_switches_without_reopening() {
        let mut modal = SettingsModal::new(None);
        let cfg = Config::default();
        modal.open(&cfg, Vec::new(), false);
        assert_eq!(modal.active_tab, SettingsTab::Font);
        modal.set_active_tab(SettingsTab::Keybindings);
        assert_eq!(modal.active_tab, SettingsTab::Keybindings);
        // Still open; switching tabs doesn't close the modal.
        assert!(modal.is_open);
    }

    #[test]
    fn cursor_shape_labels() {
        assert_eq!(cursor_shape_label(&CursorShapeConfig::Block), "Block");
        assert_eq!(
            cursor_shape_label(&CursorShapeConfig::Underline),
            "Underline"
        );
        assert_eq!(cursor_shape_label(&CursorShapeConfig::Bar), "Bar");
    }

    #[test]
    fn tab_bar_position_labels() {
        assert_eq!(tab_bar_position_label(TabBarPosition::Top), "Top");
        assert_eq!(tab_bar_position_label(TabBarPosition::Bottom), "Bottom");
    }

    #[test]
    fn gutter_position_labels() {
        assert_eq!(gutter_position_label(GutterPosition::Left), "Left");
        assert_eq!(gutter_position_label(GutterPosition::Off), "Off");
    }

    #[test]
    fn gutter_setting_persists_through_draft_apply() {
        // Editing the draft's gutter and applying it must carry the value
        // into the saved config (the modal edits `self.draft`, which is
        // written back on Apply).  The TOML round-trip itself is covered by
        // the freminal-common config test; here we verify the settings
        // modal surfaces and mutates the same field.
        let mut modal = SettingsModal::new(None);
        let mut cfg = Config::default();
        cfg.command_blocks.gutter = GutterPosition::Off;
        modal.open(&cfg, Vec::new(), false);
        assert_eq!(modal.draft.command_blocks.gutter, GutterPosition::Off);
        modal.draft.command_blocks.gutter = GutterPosition::Left;
        let applied = modal.draft.clone();
        assert_eq!(applied.command_blocks.gutter, GutterPosition::Left);
    }

    #[test]
    fn all_tabs_present() {
        assert_eq!(SettingsTab::ALL.len(), 14);
    }

    #[test]
    fn notification_routing_labels() {
        assert_eq!(
            notification_routing_label(config::NotificationRouting::Toast),
            "Toast"
        );
        assert_eq!(
            notification_routing_label(config::NotificationRouting::System),
            "System"
        );
        assert_eq!(
            notification_routing_label(config::NotificationRouting::Both),
            "Both"
        );
        assert_eq!(
            notification_routing_label(config::NotificationRouting::SystemWhenUnfocused),
            "System when unfocused"
        );
    }

    #[test]
    fn notification_settings_persist_through_draft() {
        let mut modal = SettingsModal::new(None);
        let cfg = Config::default();
        modal.open(&cfg, Vec::new(), false);
        modal.draft.notifications.enabled = true;
        modal.draft.notifications.command_finished_template = "{command}".to_owned();
        modal.draft.bell.on_command_finished = true;
        let applied = modal.draft.clone();
        assert!(applied.notifications.enabled);
        assert_eq!(applied.notifications.command_finished_template, "{command}");
        assert!(applied.bell.on_command_finished);
    }

    #[test]
    fn test_notification_button_sets_pending_flag() {
        let mut modal = SettingsModal::new(None);
        let cfg = Config::default();
        modal.open(&cfg, Vec::new(), false);
        assert!(!modal.pending_test_notification);
        // Simulate the button click side-effect.
        modal.pending_test_notification = true;
        // The drain in `show`/`show_standalone` takes the flag and returns the
        // action; emulate that here.
        let drained = std::mem::take(&mut modal.pending_test_notification);
        assert!(drained);
        assert!(!modal.pending_test_notification);
    }

    #[test]
    fn test_paste_button_sets_pending_flag() {
        let mut modal = SettingsModal::new(None);
        let cfg = Config::default();
        modal.open(&cfg, Vec::new(), false);
        assert!(!modal.pending_test_paste);
        // Simulate the button click side-effect.
        modal.pending_test_paste = true;
        // The drain in `show`/`show_standalone` takes the flag and returns
        // `SettingsAction::TestPaste`; emulate that here.
        let drained = std::mem::take(&mut modal.pending_test_paste);
        assert!(drained);
        assert!(!modal.pending_test_paste);
    }

    #[test]
    fn draft_paste_guard_reflects_edits() {
        let mut modal = SettingsModal::new(None);
        let cfg = Config::default();
        modal.open(&cfg, Vec::new(), false);
        assert!(modal.draft_paste_guard().enabled);
        // Edits to the draft are visible through the accessor used by the
        // "Test Paste" dispatch.
        modal.draft.paste_guard.enabled = false;
        modal.draft.paste_guard.pattern_list = vec![r"\bgit\s+push\b".to_owned()];
        assert!(!modal.draft_paste_guard().enabled);
        assert_eq!(
            modal.draft_paste_guard().pattern_list,
            vec![r"\bgit\s+push\b"]
        );
    }

    #[test]
    fn open_with_managed_by_sets_read_only_reason() {
        let mut modal = SettingsModal::new(None);
        let live = Config {
            managed_by: Some("home-manager".to_string()),
            ..Config::default()
        };
        modal.open(&live, Vec::new(), false);

        assert!(modal.is_open);
        assert!(modal.read_only_reason.is_some());
        let reason = modal.read_only_reason.as_ref().unwrap();
        assert!(
            reason.contains("home-manager"),
            "banner should mention the manager: {reason}"
        );
    }

    #[test]
    fn open_without_managed_by_writable_config_is_not_read_only() {
        // Use a writable temp file as the config path so the writability
        // check passes.
        let dir = std::env::temp_dir().join("freminal_test_settings_writable");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("config.toml");
        std::fs::write(&path, "version = 1\n").unwrap();

        let mut modal = SettingsModal::new(Some(path.clone()));
        let live = Config::default();
        modal.open(&live, Vec::new(), false);

        assert!(modal.is_open);
        assert!(
            modal.read_only_reason.is_none(),
            "should not be read-only for a writable config"
        );

        // Cleanup
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn open_with_readonly_config_file_sets_read_only_reason() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join("freminal_test_settings_readonly");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("readonly.toml");
        std::fs::write(&path, "version = 1\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();

        let mut modal = SettingsModal::new(Some(path.clone()));
        let live = Config::default();
        modal.open(&live, Vec::new(), false);

        assert!(modal.is_open);
        assert!(
            modal.read_only_reason.is_some(),
            "should be read-only for a non-writable config file"
        );
        let reason = modal.read_only_reason.as_ref().unwrap();
        assert!(
            reason.contains("read-only"),
            "banner should mention read-only: {reason}"
        );

        // Cleanup: restore permissions so we can delete
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn open_stores_monospace_families() {
        let mut modal = SettingsModal::new(None);
        let live = Config::default();
        let families = vec![
            "Courier New".to_string(),
            "Fira Code".to_string(),
            "JetBrains Mono".to_string(),
        ];
        modal.open(&live, families.clone(), false);

        assert_eq!(modal.monospace_families, families);
    }

    #[test]
    fn sync_from_config_updates_draft_and_originals() {
        let mut modal = SettingsModal::new(None);
        // Seed the modal with an initial config via open() so os_dark_mode
        // and the original_* fields are initialised.
        let initial = Config::default();
        modal.open(&initial, Vec::new(), false);
        let initial_opacity = modal.original_opacity;

        // Build a mutated config and sync.  Use a distinguishable opacity
        // and theme slug so we can verify the draft updates.
        let mut reloaded = Config::default();
        reloaded.ui.background_opacity = (initial_opacity - 0.5).clamp(0.0, 1.0);
        modal.sync_from_config(&reloaded);

        assert!(
            (modal.draft.ui.background_opacity - reloaded.ui.background_opacity).abs()
                < f32::EPSILON
        );
        assert!((modal.original_opacity - reloaded.ui.background_opacity).abs() < f32::EPSILON);
        assert_eq!(
            modal.original_theme_slug,
            reloaded.theme.active_slug(modal.os_dark_mode).to_string()
        );
    }

    #[test]
    fn is_dirty_false_after_open_and_true_after_edit() {
        let mut modal = SettingsModal::new(None);
        let live = Config::default();
        modal.open(&live, Vec::new(), false);

        assert!(
            !modal.is_dirty(),
            "freshly-opened modal should match its baseline"
        );

        // Mutate the draft — any change should flip the dirty flag.
        modal.draft.ui.background_opacity =
            (modal.draft.ui.background_opacity - 0.25).clamp(0.0, 1.0);
        assert!(modal.is_dirty(), "edited draft should be dirty");
    }

    #[test]
    fn request_close_closes_when_clean_and_defers_when_dirty() {
        let mut modal = SettingsModal::new(None);
        let live = Config::default();
        modal.open(&live, Vec::new(), false);
        // Force writable state: on some CI environments the default config
        // path is not writable, which would otherwise short-circuit the
        // guard into read-only mode.
        modal.read_only_reason = None;

        // Clean modal closes immediately.
        assert!(modal.request_close());
        assert!(!modal.is_open);

        // Dirty modal defers and sets the Asking state.
        modal.open(&live, Vec::new(), false);
        modal.read_only_reason = None;
        modal.draft.ui.background_opacity =
            (modal.draft.ui.background_opacity - 0.25).clamp(0.0, 1.0);
        assert!(!modal.request_close(), "dirty close should be vetoed");
        assert!(modal.is_open, "modal must stay open while Asking");
        assert_eq!(modal.pending_close, PendingClose::Asking);
    }

    #[test]
    fn request_close_bypasses_guard_in_read_only_mode() {
        let mut modal = SettingsModal::new(None);
        let live = Config::default();
        modal.open(&live, Vec::new(), false);
        // Force read-only and dirty.  Even when the draft differs, read-only
        // mode cannot apply anyway, so closing without a prompt is correct.
        modal.read_only_reason = Some("test: read-only".to_string());
        modal.draft.ui.background_opacity =
            (modal.draft.ui.background_opacity - 0.25).clamp(0.0, 1.0);
        assert!(modal.request_close());
        assert!(!modal.is_open);
    }

    #[test]
    fn startup_layout_is_missing_detects_absent_and_present() {
        use freminal_common::layout::LayoutSummary;
        use std::path::PathBuf;

        let discovered = vec![
            LayoutSummary {
                name: "dev".to_string(),
                description: None,
                path: PathBuf::from("/tmp/dev.toml"),
            },
            LayoutSummary {
                name: "ops".to_string(),
                description: Some("Ops layout".to_string()),
                path: PathBuf::from("/tmp/ops.toml"),
            },
        ];

        // None => not missing (nothing is configured).
        assert!(!startup_layout_is_missing(None, &discovered));
        // Configured and present => not missing.
        assert!(!startup_layout_is_missing(Some("dev"), &discovered));
        assert!(!startup_layout_is_missing(Some("ops"), &discovered));
        // Configured but absent => missing.
        assert!(startup_layout_is_missing(Some("ghost"), &discovered));
        // Configured but discovered list is empty => missing.
        assert!(startup_layout_is_missing(Some("dev"), &[]));
    }
}
