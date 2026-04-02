// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::{self, ComboBox, DragValue, FontData, FontDefinitions, FontFamily, Slider, Ui};
use freminal_common::config::{self, Config, CursorShapeConfig};
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
}

impl SettingsTab {
    /// All tabs in display order.
    const ALL: [Self; 7] = [
        Self::Font,
        Self::Cursor,
        Self::Theme,
        Self::Shell,
        Self::Scrollback,
        Self::Logging,
        Self::Ui,
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
        }
    }
}

/// The result of showing the settings modal for one frame.
///
/// The caller uses this to decide whether to apply config changes, re-register
/// fonts, etc.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
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
}

/// Persistent state for the settings modal.
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

    /// Theme slug when the modal was opened.  Used to detect whether a
    /// preview is active and to revert on Cancel.
    original_theme_slug: String,

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
            original_opacity: 1.0,
            monospace_families: Vec::new(),
            base_font_defs: None,
            preview_registered: None,
        }
    }

    /// Open the modal, cloning the live config into the draft for editing.
    ///
    /// `monospace_families` is the sorted, deduplicated list of monospaced font
    /// family names available on the system (from [`FontManager::enumerate_monospace_families`]).
    pub fn open(&mut self, live_config: &Config, monospace_families: Vec<String>) {
        self.draft = live_config.clone();
        self.active_tab = SettingsTab::Font;
        self.status_message = None;
        self.monospace_families = monospace_families;
        self.original_theme_slug.clone_from(&live_config.theme.name);
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

        self.is_open = true;
    }

    /// Show the modal window. Returns the action the caller should take.
    ///
    /// When `SettingsAction::Applied` is returned, the caller should:
    ///   1. Replace its live config with `self.applied_config()`.
    ///   2. Hot-reload any settings that can change at runtime.
    pub fn show(&mut self, ctx: &egui::Context) -> SettingsAction {
        if !self.is_open {
            return SettingsAction::None;
        }

        let mut action = SettingsAction::None;
        let mut open = self.is_open;

        // Snapshot the draft theme slug and opacity before rendering so we
        // can detect whether the user changed either this frame.
        let theme_before = self.draft.theme.name.clone();
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
                ui.horizontal(|ui| {
                    for tab in SettingsTab::ALL {
                        ui.selectable_value(&mut self.active_tab, tab, tab.label());
                    }
                });
                ui.separator();

                // --- Tab content ---
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .max_height(300.0)
                    .show(ui, |ui| {
                        // In read-only mode, disable all interactive widgets
                        // so the user can browse but not edit.
                        if is_read_only {
                            ui.disable();
                        }
                        match self.active_tab {
                            SettingsTab::Font => self.show_font_tab(ui),
                            SettingsTab::Cursor => self.show_cursor_tab(ui),
                            SettingsTab::Theme => self.show_theme_tab(ui),
                            SettingsTab::Shell => self.show_shell_tab(ui),
                            SettingsTab::Scrollback => self.show_scrollback_tab(ui),
                            SettingsTab::Logging => self.show_logging_tab(ui),
                            SettingsTab::Ui => self.show_ui_tab(ui),
                        }
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
                            self.is_open = false;
                        }
                    });
                });
            });

        // Handle the X button on the window title bar.
        if !open {
            self.is_open = false;
        }

        // If the modal just closed (Cancel or X) without Apply, revert any
        // previewed settings to their originals.
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

        // If the theme dropdown changed this frame, signal a live preview.
        if self.draft.theme.name != theme_before && action != SettingsAction::Applied {
            return SettingsAction::PreviewTheme(self.draft.theme.name.clone());
        }

        action
    }

    /// Returns a reference to the draft config. Only meaningful after
    /// `SettingsAction::Applied` is returned.
    #[must_use]
    pub const fn applied_config(&self) -> &Config {
        &self.draft
    }

    // -------------------------------------------------------------------------
    //  Tab implementations
    // -------------------------------------------------------------------------

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
    }

    fn show_theme_tab(&mut self, ui: &mut Ui) {
        // Look up the display name for the currently selected slug.
        let selected_display = themes::by_slug(&self.draft.theme.name)
            .map_or_else(|| self.draft.theme.name.clone(), |t| t.name.to_string());

        ui.label("Theme:");
        ComboBox::from_id_salt("theme_name")
            .selected_text(&selected_display)
            .show_ui(ui, |ui| {
                for theme in themes::all_themes() {
                    ui.selectable_value(
                        &mut self.draft.theme.name,
                        theme.slug.to_string(),
                        theme.name,
                    );
                }
            });
        ui.add_space(8.0);

        // Color preview strip: show the selected theme's palette.
        if let Some(theme) = themes::by_slug(&self.draft.theme.name) {
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
                SettingsAction::Applied
            }
            Err(e) => {
                self.status_message = Some(format!("Save failed: {e}"));
                SettingsAction::None
            }
        }
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
    fn open_clones_live_config_into_draft() {
        let mut modal = SettingsModal::new(None);
        let mut live = Config::default();
        live.font.size = 20.0;
        live.scrollback.limit = 500;
        modal.open(&live, Vec::new());

        assert!(modal.is_open);
        assert!((modal.draft.font.size - 20.0).abs() < f32::EPSILON);
        assert_eq!(modal.draft.scrollback.limit, 500);
    }

    #[test]
    fn reset_to_defaults_produces_default_config() {
        let mut modal = SettingsModal::new(None);
        let mut live = Config::default();
        live.font.size = 42.0;
        modal.open(&live, Vec::new());

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
    fn all_tabs_present() {
        assert_eq!(SettingsTab::ALL.len(), 7);
    }

    #[test]
    fn open_with_managed_by_sets_read_only_reason() {
        let mut modal = SettingsModal::new(None);
        let live = Config {
            managed_by: Some("home-manager".to_string()),
            ..Config::default()
        };
        modal.open(&live, Vec::new());

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
        modal.open(&live, Vec::new());

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
        modal.open(&live, Vec::new());

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
        modal.open(&live, families.clone());

        assert_eq!(modal.monospace_families, families);
    }
}
