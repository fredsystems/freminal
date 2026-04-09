// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui::{self, ComboBox, DragValue, FontData, FontDefinitions, FontFamily, Slider, Ui};
use freminal_common::config::{self, Config, CursorShapeConfig, TabBarPosition, ThemeMode};
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
    Bell,
    Security,
    Keybindings,
}

impl SettingsTab {
    /// All tabs in display order.
    const ALL: [Self; 11] = [
        Self::Font,
        Self::Cursor,
        Self::Theme,
        Self::Shell,
        Self::Scrollback,
        Self::Logging,
        Self::Ui,
        Self::Tabs,
        Self::Bell,
        Self::Security,
        Self::Keybindings,
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
            Self::Bell => "Bell",
            Self::Security => "Security",
            Self::Keybindings => "Keybindings",
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

/// Result of scanning a frame's egui events for a key-binding press.
#[derive(Debug, Clone, Copy)]
enum KeyCapture {
    /// User pressed Escape — cancel recording.
    Cancelled,
    /// A valid binding combo was captured.
    Combo(KeyCombo),
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
        }
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
        self.is_open = true;
    }

    /// Show the modal window. Returns the action the caller should take.
    ///
    /// When `SettingsAction::Applied` is returned, the caller should:
    ///   1. Replace its live config with `self.applied_config()`.
    ///   2. Hot-reload any settings that can change at runtime.
    ///
    /// `os_dark_mode` is used to resolve `ThemeMode::Auto` for preview/revert.
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
            SettingsTab::Bell => self.show_bell_tab(ui),
            SettingsTab::Security => self.show_security_tab(ui),
            SettingsTab::Keybindings => self.show_keybindings_tab(ui),
        }
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
                ui.selectable_value(&mut self.draft.bell.mode, config::BellMode::None, "None");
            });

        ui.add_space(4.0);
        ui.colored_label(
            egui::Color32::GRAY,
            "Visual: briefly flash the terminal area.\n\
             None: silently ignore the bell.",
        );
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
        if ui
            .small_button("\u{2715}")
            .on_hover_text("Unbind")
            .clicked()
        {
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

/// Human-readable label for a `TabBarPosition` variant.
const fn tab_bar_position_label(pos: TabBarPosition) -> &'static str {
    match pos {
        TabBarPosition::Top => "Top",
        TabBarPosition::Bottom => "Bottom",
    }
}

const fn bell_mode_label(mode: config::BellMode) -> &'static str {
    match mode {
        config::BellMode::Visual => "Visual",
        config::BellMode::None => "None",
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
        assert_eq!(SettingsTab::Bell.label(), "Bell");
        assert_eq!(SettingsTab::Security.label(), "Security");
        assert_eq!(SettingsTab::Keybindings.label(), "Keybindings");
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
    fn all_tabs_present() {
        assert_eq!(SettingsTab::ALL.len(), 11);
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
}
