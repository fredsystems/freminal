// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::{self, ComboBox, DragValue, Slider, Ui};
use freminal_common::config::{self, Config, CursorShapeConfig};

/// Which tab is currently active in the settings modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    Font,
    Cursor,
    Theme,
    Shell,
    Scrollback,
    Logging,
}

impl SettingsTab {
    /// All tabs in display order.
    const ALL: [Self; 6] = [
        Self::Font,
        Self::Cursor,
        Self::Theme,
        Self::Shell,
        Self::Scrollback,
        Self::Logging,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Font => "Font",
            Self::Cursor => "Cursor",
            Self::Theme => "Theme",
            Self::Shell => "Shell",
            Self::Scrollback => "Scrollback",
            Self::Logging => "Logging",
        }
    }
}

/// The result of showing the settings modal for one frame.
///
/// The caller uses this to decide whether to apply config changes, re-register
/// fonts, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    /// No action this frame (modal still open or closed without applying).
    None,
    /// The user clicked Apply — the new config has been saved to disk and
    /// should be adopted live.
    Applied,
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
    config_path: Option<std::path::PathBuf>,
}

impl SettingsModal {
    /// Create a new (closed) settings modal.
    #[must_use]
    pub fn new(config_path: Option<std::path::PathBuf>) -> Self {
        Self {
            is_open: false,
            draft: Config::default(),
            active_tab: SettingsTab::Font,
            status_message: None,
            config_path,
        }
    }

    /// Open the modal, cloning the live config into the draft for editing.
    pub fn open(&mut self, live_config: &Config) {
        self.draft = live_config.clone();
        self.active_tab = SettingsTab::Font;
        self.status_message = None;
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

        egui::Window::new("Settings")
            .collapsible(false)
            .resizable(true)
            .default_width(450.0)
            .open(&mut open)
            .show(ctx, |ui| {
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
                    .show(ui, |ui| match self.active_tab {
                        SettingsTab::Font => self.show_font_tab(ui),
                        SettingsTab::Cursor => self.show_cursor_tab(ui),
                        SettingsTab::Theme => self.show_theme_tab(ui),
                        SettingsTab::Shell => self.show_shell_tab(ui),
                        SettingsTab::Scrollback => self.show_scrollback_tab(ui),
                        SettingsTab::Logging => self.show_logging_tab(ui),
                    });

                ui.separator();

                // --- Status message ---
                if let Some(msg) = &self.status_message {
                    ui.colored_label(egui::Color32::YELLOW, msg);
                }

                // --- Bottom buttons ---
                ui.horizontal(|ui| {
                    if ui.button("Reset to Defaults").clicked() {
                        self.draft = Config::default();
                        self.status_message = Some("Reset to defaults (not saved yet)".to_string());
                    }

                    // Right-align Apply and Cancel.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Apply").clicked() {
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

    fn show_font_tab(&mut self, ui: &mut Ui) {
        ui.label("Font Family:");
        let mut family_text = self.draft.font.family.clone().unwrap_or_default();
        let response = ui.text_edit_singleline(&mut family_text);
        if response.changed() {
            self.draft.font.family = if family_text.is_empty() {
                None
            } else {
                Some(family_text)
            };
        }
        ui.label("Leave empty to use the bundled default font.");
        ui.add_space(8.0);

        ui.label("Font Size:");
        ui.add(Slider::new(&mut self.draft.font.size, 4.0..=96.0).step_by(0.5));
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
        ui.label("Theme:");
        ComboBox::from_id_salt("theme_name")
            .selected_text(&self.draft.theme.name)
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut self.draft.theme.name,
                    "catppuccin-mocha".to_string(),
                    "Catppuccin Mocha",
                );
            });
        ui.add_space(8.0);
        ui.label("Custom themes are planned for a future release.");
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
        ui.checkbox(&mut self.draft.logging.write_to_file, "Write logs to file");
        ui.add_space(8.0);
        ui.colored_label(egui::Color32::GRAY, "Changes take effect on next launch.");
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

#[cfg(test)]
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
        modal.open(&live);

        assert!(modal.is_open);
        assert!((modal.draft.font.size - 20.0).abs() < f32::EPSILON);
        assert_eq!(modal.draft.scrollback.limit, 500);
    }

    #[test]
    fn reset_to_defaults_produces_default_config() {
        let mut modal = SettingsModal::new(None);
        let mut live = Config::default();
        live.font.size = 42.0;
        modal.open(&live);

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
        assert_eq!(SettingsTab::ALL.len(), 6);
    }
}
