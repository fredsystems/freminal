// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Style-profile tuning gallery (subtask 112.3a).
//!
//! A standalone developer tool — **not** part of the shipping product — used
//! to dial in the Modern / Retro [`StyleProfile`] geometry and lock the chrome
//! aesthetic before it is applied app-wide (112.4) and rolled out per surface
//! (112.6–112.9).
//!
//! Run with:
//!
//! ```text
//! cargo run -p freminal --example chrome_gallery
//! ```
//!
//! The gallery renders a representative slice of chrome (buttons, combo box,
//! slider, separator, tab pair, bordered frame, toast frame, selected text)
//! styled by the **real**
//! [`freminal::gui::chrome_style::build_visuals`](freminal::gui::chrome_style::build_visuals)
//! mapping — never a copy — so whatever is tuned here is exactly what the
//! product will render. Live controls edit an in-memory [`GuiTheme`] draft;
//! the styled region restyles every frame.
//!
//! When the look is signed off, copy the geometry values back into
//! [`StyleProfile::defaults`] (in `freminal-common`) and record them in
//! `PLAN_VERSION_100.md` under "112.3a — locked baselines".

use freminal::gui::chrome_style::build_visuals;
use freminal_common::gui_theme::{GuiTheme, StyleProfile};
use freminal_common::themes::{self, ThemePalette};
use freminal_windowing::{App, WindowConfig, WindowHandle, WindowId};

/// All built-in themes, in registry order, so every theme can be audited
/// (112.3f/g) — not a curated subset.
fn palettes() -> &'static [&'static ThemePalette] {
    themes::all_themes()
}

/// Gallery application state.
struct ChromeGallery {
    /// The geometry draft being tuned. Edited live by the controls.
    draft: GuiTheme,
    /// Index into [`palettes`] of the palette currently driving the preview.
    palette_idx: usize,
    /// Background opacity passed to `build_visuals` (`panel_fill` alpha).
    bg_opacity: f32,
    /// `true` = normal display; `false` = reverse-video (forces white fills).
    normal_display: bool,
    /// Sample state for the gallery's combo box.
    combo_choice: usize,
    /// Sample state for the gallery's slider.
    slider_value: f32,
}

impl Default for ChromeGallery {
    fn default() -> Self {
        Self {
            draft: StyleProfile::Modern.defaults(),
            palette_idx: 0,
            bg_opacity: 1.0,
            normal_display: true,
            combo_choice: 0,
            slider_value: 0.5,
        }
    }
}

impl ChromeGallery {
    /// Currently selected palette.
    fn palette(&self) -> &'static ThemePalette {
        let all = palettes();
        all[self.palette_idx.min(all.len() - 1)]
    }

    /// Draw the control column (left): profile, geometry, palette, mode.
    ///
    /// These widgets render in the example window's own (default) style, not
    /// the previewed style — they are the instrument, not the specimen.
    fn controls(&mut self, ui: &mut egui::Ui) {
        ui.heading("Controls");
        ui.add_space(4.0);

        ui.label("Style profile");
        ui.horizontal(|ui| {
            if ui
                .selectable_label(self.draft.profile == StyleProfile::Modern, "Modern")
                .clicked()
            {
                self.draft = StyleProfile::Modern.defaults();
            }
            if ui
                .selectable_label(self.draft.profile == StyleProfile::Retro, "Retro")
                .clicked()
            {
                self.draft = StyleProfile::Retro.defaults();
            }
        });

        ui.separator();
        ui.label("Geometry");
        ui.add(egui::Slider::new(&mut self.draft.corner_radius, 0..=24).text("corner_radius"));
        ui.add(
            egui::Slider::new(&mut self.draft.menu_corner_radius, 0..=24)
                .text("menu_corner_radius"),
        );
        ui.add(egui::Slider::new(&mut self.draft.stroke_width, 0.0..=4.0).text("stroke_width"));
        ui.add(
            egui::Slider::new(&mut self.draft.widget_hover_expansion, 0.0..=6.0)
                .text("hover_expansion"),
        );
        ui.add(
            egui::Slider::new(&mut self.draft.item_spacing.0, 0.0..=20.0).text("item_spacing.x"),
        );
        ui.add(
            egui::Slider::new(&mut self.draft.item_spacing.1, 0.0..=20.0).text("item_spacing.y"),
        );
        ui.add(
            egui::Slider::new(&mut self.draft.window_padding, 0.0..=24.0).text("window_padding"),
        );

        ui.separator();
        ui.label(format!("Theme ({} built-in)", palettes().len()));
        egui::ComboBox::from_id_salt("palette_picker")
            .selected_text(self.palette().name)
            .show_ui(ui, |ui| {
                for (idx, palette) in palettes().iter().enumerate() {
                    ui.selectable_value(&mut self.palette_idx, idx, palette.name);
                }
            });
        // Indicate whether this theme has authored chrome roles or is
        // resolver-driven (useful during the per-theme audit).
        let authored = self.palette().chrome_surface.is_some();
        ui.label(if authored {
            "roles: authored (upstream)"
        } else {
            "roles: resolver (best-fit/contrast)"
        });

        ui.separator();
        ui.label("Display mode");
        ui.add(egui::Slider::new(&mut self.bg_opacity, 0.0..=1.0).text("bg_opacity"));
        ui.checkbox(
            &mut self.normal_display,
            "normal display (uncheck = reverse video)",
        );

        ui.separator();
        ui.label(format!(
            "draft: radius {} / menu {} / stroke {:.1} / hover {:.1} / spacing ({:.0},{:.0}) / pad {:.0}",
            self.draft.corner_radius,
            self.draft.menu_corner_radius,
            self.draft.stroke_width,
            self.draft.widget_hover_expansion,
            self.draft.item_spacing.0,
            self.draft.item_spacing.1,
            self.draft.window_padding,
        ));
    }

    /// Draw the previewed chrome (right).
    ///
    /// The previewed style is applied as the window's **global** `Visuals` in
    /// [`update`](ChromeGallery::update), not scoped locally — this is
    /// deliberate: combo-box dropdowns and context menus render in egui's
    /// top-level popup layer, which reads the *global* style and ignores a
    /// local `ui.scope`. Applying globally is the only way to faithfully
    /// preview menu/dropdown theming, and is safe here because the gallery
    /// owns its whole window.
    fn preview(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .inner_margin(self.draft.window_padding)
            .show(ui, |ui| {
                ui.heading("Preview");
                ui.add_space(4.0);

                ui.label("Buttons");
                ui.horizontal(|ui| {
                    let _ = ui.button("Default");
                    let _ = ui.button("Cancel");
                    let _ = ui.button("Apply");
                });

                ui.add_space(6.0);
                ui.label("Combo box (dropdown popup follows the theme)");
                egui::ComboBox::from_id_salt("preview_combo")
                    .selected_text(format!("Option {}", self.combo_choice + 1))
                    .show_ui(ui, |ui| {
                        for idx in 0..4 {
                            ui.selectable_value(
                                &mut self.combo_choice,
                                idx,
                                format!("Option {}", idx + 1),
                            );
                        }
                    });
                ui.add(egui::Slider::new(&mut self.slider_value, 0.0..=1.0).text("value"));

                ui.add_space(6.0);
                ui.label("Right-click the button below for a context menu:");
                ui.button("Right-click me").context_menu(|ui| {
                    let _ = ui.button("Cut");
                    let _ = ui.button("Copy");
                    let _ = ui.button("Paste");
                    ui.separator();
                    let _ = ui.button("Select all");
                });

                ui.separator();

                ui.label("Tabs");
                ui.horizontal(|ui| {
                    let _ = ui.selectable_label(true, "Active tab");
                    let _ = ui.selectable_label(false, "Inactive tab");
                    let _ = ui.selectable_label(false, "Inactive tab");
                });

                ui.add_space(6.0);
                ui.label("Bordered panel (corner radius + stroke)");
                egui::Frame::group(ui.style()).show(ui, |ui| {
                    ui.label("Panel content sits inside a stroked, rounded frame.");
                });

                ui.add_space(6.0);
                ui.label("Toast frame");
                let toast_fill = ui.visuals().widgets.inactive.bg_fill;
                egui::Frame::new()
                    .fill(toast_fill)
                    .stroke(ui.visuals().window_stroke)
                    .corner_radius(ui.visuals().window_corner_radius)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        // A single vertically-centered, content-sized row. The
                        // previous nested right_to_left layout claimed the full
                        // available width (the "huge toast" bug); a plain
                        // centered horizontal row sizes to its contents and
                        // keeps the label and close button aligned.
                        ui.horizontal(|ui| {
                            ui.label("Notification text");
                            ui.add_space(12.0);
                            let _ = ui.button("x");
                        });
                    });

                ui.add_space(6.0);
                ui.label("Text edit (extreme_bg_color surface):");
                let mut sample = String::from("the quick brown fox");
                let margin = freminal::gui::chrome_style::text_edit_margin(&self.draft);
                ui.add(egui::TextEdit::singleline(&mut sample).margin(margin));
            });
    }
}

impl App for ChromeGallery {
    fn update(
        &mut self,
        _window_id: WindowId,
        ctx: &egui::Context,
        _gl: &glow::Context,
        _handle: &WindowHandle<'_>,
    ) {
        // Apply the draft style as the window's GLOBAL visuals so that
        // top-level popup surfaces (combo dropdowns, context menus) — which
        // do not inherit a local `ui.scope` style — also follow the theme.
        // This is what 112.4 will do for the real app via the per-frame hook.
        let visuals = build_visuals(
            &self.draft,
            self.palette(),
            self.bg_opacity,
            self.normal_display,
        );
        ctx.set_visuals(visuals);
        ctx.global_style_mut(|style| {
            freminal::gui::chrome_style::apply_chrome_spacing(style, &self.draft);
        });

        // Mirror the app's root-Ui + `show(ui, …)` idiom (the top-level
        // `CentralPanel::default().show(ctx, …)` form is gone in egui 0.35;
        // panels now take a `&mut Ui`).
        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("chrome_gallery_root"),
            egui::UiBuilder::default(),
        );
        egui::CentralPanel::default().show(&mut root_ui, |ui| {
            ui.columns(2, |cols| {
                self.controls(&mut cols[0]);
                self.preview(&mut cols[1]);
            });
        });
    }

    fn on_window_created(
        &mut self,
        _window_id: WindowId,
        _ctx: &egui::Context,
        _handle: &WindowHandle<'_>,
        _inner_size: (u32, u32),
    ) {
    }

    fn on_close_requested(&mut self, _window_id: WindowId) -> bool {
        true
    }

    fn clear_color(&self, _window_id: WindowId) -> [f32; 4] {
        [0.08, 0.08, 0.10, 1.0]
    }
}

fn main() -> Result<(), freminal_windowing::Error> {
    let config = WindowConfig {
        title: "Freminal — Chrome Style Gallery (112.3a)".to_owned(),
        inner_size: Some((1100, 760)),
        position: None,
        transparent: false,
        icon: None,
        app_id: Some("freminal-chrome-gallery".to_owned()),
    };
    freminal_windowing::run(config, ChromeGallery::default())
}
