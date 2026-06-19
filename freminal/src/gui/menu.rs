// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use egui;
use freminal_common::config::TabTitlePolicy;
use freminal_common::keybindings::KeyAction;

use super::TabBarAction;
use super::icons::ChromeIcon;
use super::tabs::Tab;
use super::window::PerWindowState;

impl super::FreminalGui {
    /// Create a `Button` with the shortcut label for `action` from the active
    /// binding map, using platform-canonical modifier symbols.
    fn menu_button_for(&self, label: &str, action: KeyAction) -> egui::Button<'_> {
        let btn = egui::Button::new(label);
        if let Some(combo) = self.binding_map.combo_for(action) {
            btn.shortcut_text(combo.display_platform())
        } else {
            btn
        }
    }
    /// Show the top menu bar.
    ///
    /// Contains a "Freminal" menu with Settings and Quit entries, a "Tab"
    /// menu with tab management actions.
    ///
    /// Returns `(action, any_menu_open)` — the second element is `true`
    /// when any dropdown menu is currently expanded, so the caller can
    /// suppress terminal input and prevent the dismiss-click from leaking
    /// through to the PTY.
    pub(super) fn show_menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        win: &mut PerWindowState,
        window_id: super::WindowId,
    ) -> (TabBarAction, bool) {
        let mut menu_action = TabBarAction::None;
        let mut any_menu_open = false;
        // egui's default `MenuBar` style (`egui::menu_style`) sets
        // `button_padding = (2.0, 0.0)`, leaving the top-level menu items with
        // no vertical breathing room — the bar feels cramped. Override the
        // bar's button style to keep the menu look (transparent inactive fill,
        // no widget strokes) while restoring vertical padding so the items have
        // comfortable space above and below the label, like a traditional menu.
        let bar_style = |style: &mut egui::Style| {
            egui::containers::menu::menu_style(style);
            // Restore vertical (and a little horizontal) padding on the
            // top-level bar buttons.
            style.spacing.button_padding = egui::vec2(8.0, 6.0);
        };
        egui::MenuBar::new().style(bar_style).ui(ui, |ui| {
            self.show_menu_bar_contents(ui, win, window_id, &mut menu_action, &mut any_menu_open);
        });
        (menu_action, any_menu_open)
    }

    /// Populate the menu bar with all top-level menus plus right-aligned
    /// status indicators. Extracted from `show_menu_bar` to keep the entry
    /// point under clippy's `too_many_lines` threshold.
    fn show_menu_bar_contents(
        &mut self,
        ui: &mut egui::Ui,
        win: &mut PerWindowState,
        window_id: super::WindowId,
        menu_action: &mut TabBarAction,
        any_menu_open: &mut bool,
    ) {
        let freminal_resp = ui.menu_button("Freminal", |ui| {
            if ui
                .add(self.menu_button_for("Settings...", KeyAction::OpenSettings))
                .clicked()
            {
                if self.settings_window_id.is_some() {
                    // Settings window already exists — focus it.
                    self.pending_focus_settings = true;
                } else if !self.settings_modal.is_open && !self.pending_settings_window {
                    let families = win.terminal_widget.monospace_families();
                    self.settings_modal
                        .open(&self.config, families, win.os_dark_mode);
                    self.settings_modal
                        .set_base_font_defs(win.terminal_widget.base_font_defs().clone());
                    self.settings_owner = Some(window_id);
                    self.pending_settings_window = true;
                }
                ui.close();
            }

            ui.separator();

            if ui.button("Quit").clicked() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        if freminal_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let edit_resp = ui.menu_button("Edit", |ui| {
            self.show_edit_menu(ui, win);
        });
        if edit_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let tab_resp = ui.menu_button("Tab", |ui| {
            *menu_action = self.show_tab_menu(ui, win);
        });
        if tab_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let pane_resp = ui.menu_button("Pane", |ui| {
            self.show_pane_menu(ui, win);
        });
        if pane_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let window_resp = ui.menu_button("Window", |ui| {
            if ui
                .add(self.menu_button_for("New Window", KeyAction::NewWindow))
                .clicked()
            {
                win.pending_new_window = true;
                ui.close();
            }
        });
        if window_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let layout_resp = ui.menu_button("Layouts", |ui| {
            self.show_layouts_menu(ui, win, window_id);
        });
        if layout_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let session_resp = ui.menu_button("Session", |ui| {
            self.show_session_menu(ui, win);
        });
        if session_resp.inner.is_some() {
            *any_menu_open = true;
        }

        let help_resp = ui.menu_button("Help", |ui| {
            self.show_help_menu(ui);
        });
        if help_resp.inner.is_some() {
            *any_menu_open = true;
        }

        // Right-aligned status indicators (recording state, password lock).
        // The password-prompt lock indicator is shown in the menu bar
        // (which is always visible) so it works regardless of tab bar
        // visibility. The REC indicator shares the same right-aligned
        // layout.
        let show_lock = self.config.security.password_indicator
            && win
                .tabs
                .active_tab()
                .active_pane()
                .is_some_and(|p| p.echo_off.load(std::sync::atomic::Ordering::Relaxed));
        let show_rec = self.is_recording();

        if show_lock || show_rec {
            // Semantic indicator colors come from the centralized themed
            // Visuals (set in 112.4): the lock is a "warning"-class indicator
            // (palette yellow), the REC dot an "error"-class indicator
            // (palette red).  No hard-coded hues — they follow the active theme.
            let warn_color = ui.visuals().warn_fg_color;
            let error_color = ui.visuals().error_fg_color;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if show_lock {
                    ui.label(ChromeIcon::Lock.rich_text_colored(warn_color));
                }
                if show_rec {
                    // Recording dot + "REC" label; kept short so it does not
                    // crowd the menu bar. The dot is a bundled icon; "REC" stays
                    // plain text.
                    ui.label(
                        ChromeIcon::RecordDot
                            .rich_text_colored(error_color)
                            .strong(),
                    );
                    ui.label(egui::RichText::new("REC").color(error_color).strong());
                }
            });
        }
    }

    /// Render the "Layouts" dropdown menu contents.
    fn show_layouts_menu(
        &mut self,
        ui: &mut egui::Ui,
        _win: &mut PerWindowState,
        _window_id: super::WindowId,
    ) {
        if ui
            .add(self.menu_button_for("Save Layout", KeyAction::SaveLayout))
            .clicked()
        {
            // Open the floating name-entry prompt (rendered in update() via
            // show_save_layout_prompt) and close the dropdown.
            self.pending_save_layout = Some(String::new());
            self.save_layout_prompt_just_opened = true;
            ui.close();
        }

        if !self.discovered_layouts.is_empty() {
            ui.separator();
            // Clone to avoid holding an immutable borrow of `self` while
            // the loop body needs `&mut self`.
            let layouts = self.discovered_layouts.clone();
            for summary in &layouts {
                if ui.button(&summary.name).clicked() {
                    match freminal_common::layout::Layout::from_file(&summary.path).and_then(|l| {
                        l.apply_variables(&[], &std::collections::HashMap::new())
                            .resolve()
                    }) {
                        Ok(resolved) => {
                            self.pending_load_layout = Some(resolved);
                        }
                        Err(e) => {
                            tracing::error!("Failed to load layout '{}': {e}", summary.name);
                            self.push_error_toast(
                                "Failed to load layout",
                                Some(format!("{}: {e}", summary.name)),
                            );
                        }
                    }
                    ui.close();
                }
            }
        }
    }

    /// Render the "Edit" dropdown menu contents.
    ///
    /// Each item enqueues a `KeyAction` onto `win.pending_menu_actions`,
    /// which is drained in `update()` after the menu bar finishes rendering.
    /// See `FreminalGui::dispatch_menu_action` for the receiving side.
    fn show_edit_menu(&self, ui: &mut egui::Ui, win: &mut PerWindowState) {
        if ui
            .add(self.menu_button_for("Copy", KeyAction::Copy))
            .clicked()
        {
            win.pending_menu_actions.push(KeyAction::Copy);
            ui.close();
        }
        if ui
            .add(self.menu_button_for("Paste", KeyAction::Paste))
            .clicked()
        {
            win.pending_menu_actions.push(KeyAction::Paste);
            ui.close();
        }
        if ui
            .add(self.menu_button_for("Select All", KeyAction::SelectAll))
            .clicked()
        {
            win.pending_menu_actions.push(KeyAction::SelectAll);
            ui.close();
        }
        ui.separator();
        if ui
            .add(self.menu_button_for("Find...", KeyAction::OpenSearch))
            .clicked()
        {
            win.pending_menu_actions.push(KeyAction::OpenSearch);
            ui.close();
        }
    }

    /// Render the "Help" dropdown menu contents.
    ///
    /// Contains three entries:
    /// - "About Freminal" — opens the in-app About dialog
    /// - "Report Issue..." — opens the GitHub issue tracker in the user's
    ///   default browser
    /// - "Keybindings..." — opens the Settings Modal focused on the
    ///   Keybindings tab
    ///
    /// None of these items have keyboard shortcuts, so they use plain
    /// `ui.button(...)` rather than `menu_button_for`.
    fn show_help_menu(&mut self, ui: &mut egui::Ui) {
        if ui.button("About Freminal").clicked() {
            self.about_window_open = true;
            ui.close();
        }
        ui.separator();
        if ui.button("Report Issue...").clicked() {
            let url = "https://github.com/fredsystems/freminal/issues/new";
            if let Err(e) = open::that(url) {
                tracing::error!("Failed to open issue tracker URL '{url}': {e}");
                self.push_error_toast("Failed to open issue tracker", Some(format!("{e}")));
            }
            ui.close();
        }
        if ui.button("Keybindings...").clicked() {
            self.pending_open_keybindings = true;
            ui.close();
        }
        ui.separator();
        if ui.button("Show Welcome...").clicked() {
            self.welcome.open();
            ui.close();
        }
    }

    /// Render the "Session" dropdown menu contents.
    ///
    /// Contains recording controls.  The toggle label reflects current
    /// state: "Start Recording" when idle, "Stop Recording" when active.
    /// When a recording is in progress, the destination path is shown as
    /// a dimmed, non-interactive line below the toggle so the user can
    /// see where the file is being written.
    fn show_session_menu(&mut self, ui: &mut egui::Ui, win: &mut PerWindowState) {
        let recording = self.is_recording();
        let label = if recording {
            "Stop Recording"
        } else {
            "Start Recording"
        };
        if ui
            .add(self.menu_button_for(label, KeyAction::ToggleRecording))
            .clicked()
        {
            self.toggle_recording();
            ui.close();
        }

        if recording && let Some(path) = self.recording_path.as_ref() {
            ui.separator();
            ui.add_enabled(
                false,
                egui::Label::new(
                    egui::RichText::new(format!("Recording to: {}", path.display()))
                        .small()
                        .weak(),
                ),
            );
        }

        ui.separator();
        // Re-reads `config.toml` from disk and applies every change live.
        // See `FreminalGui::reload_config_from_disk` for behaviour, including
        // the no-op toast when no config path is associated with the session.
        let reload_enabled = self.config_path.is_some();
        let reload_resp = ui.add_enabled(
            reload_enabled,
            self.menu_button_for("Reload Config", KeyAction::ReloadConfig),
        );
        if reload_resp.clicked() {
            win.pending_menu_actions.push(KeyAction::ReloadConfig);
            ui.close();
        }
        if !reload_enabled {
            reload_resp.on_disabled_hover_text("No config file is associated with this session.");
        }
    }

    /// Show the "About Freminal" floating dialog.
    ///
    /// Rendered every frame when `about_window_open` is `true`.  Displays
    /// the package version, the git-describe string (build hash), a short
    /// description, and a Close button.  The dialog is anchored to the
    /// window center and is neither resizable nor collapsible.
    pub(super) fn show_about_window(&mut self, ctx: &egui::Context) {
        if !self.about_window_open {
            return;
        }

        let mut open = true;
        egui::Window::new("About Freminal")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .open(&mut open)
            .show(ctx, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space(4.0);
                    ui.heading("Freminal");
                    ui.add_space(2.0);
                    ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                    ui.label(format!(
                        "Build {}",
                        freminal_terminal_emulator::GIT_DESCRIBE
                    ));
                    ui.add_space(6.0);
                    ui.label("A modern terminal emulator written in Rust.");
                    ui.add_space(2.0);
                    ui.label("Licensed under the MIT License.");
                    ui.add_space(2.0);
                    // Attribution for vendored code and bundled fonts/assets
                    // lives in ATTRIBUTIONS.md (the single source of truth);
                    // link to it rather than duplicating license text here.
                    // A hyperlink opens the system browser and needs no
                    // keyboard focus, so it does not affect modal input
                    // suppression (the About window is already registered in
                    // `ui_overlay_open`).
                    ui.hyperlink_to(
                        "Third-party attributions",
                        "https://github.com/fredsystems/freminal/blob/main/ATTRIBUTIONS.md",
                    );
                    ui.add_space(10.0);
                    if ui.button("Close").clicked() {
                        self.about_window_open = false;
                    }
                });
            });

        // Honor the window's own close button (the `X` in the title bar).
        if !open {
            self.about_window_open = false;
        }
    }

    /// Render the "Tab" dropdown menu contents.
    ///
    /// Returns a `TabBarAction` if the user clicked a tab management item.
    fn show_tab_menu(&self, ui: &mut egui::Ui, win: &PerWindowState) -> TabBarAction {
        if ui
            .add(self.menu_button_for("New Tab", KeyAction::NewTab))
            .clicked()
        {
            ui.close();
            return TabBarAction::NewTab;
        }

        let active = win.tabs.active_index();
        let can_close = win.tabs.tab_count() > 1;
        if ui
            .add_enabled(
                can_close,
                self.menu_button_for("Close Tab", KeyAction::CloseTab),
            )
            .clicked()
        {
            ui.close();
            return TabBarAction::Close(active);
        }

        ui.separator();

        if ui
            .add(self.menu_button_for("Next Tab", KeyAction::NextTab))
            .clicked()
        {
            let next = (active + 1) % win.tabs.tab_count();
            ui.close();
            return TabBarAction::SwitchTo(next);
        }

        if ui
            .add(self.menu_button_for("Previous Tab", KeyAction::PrevTab))
            .clicked()
        {
            let count = win.tabs.tab_count();
            let prev = if active == 0 { count - 1 } else { active - 1 };
            ui.close();
            return TabBarAction::SwitchTo(prev);
        }

        TabBarAction::None
    }

    /// Render the "Pane" dropdown menu contents.
    ///
    /// Extracted from `show_menu_bar` to keep that function under the
    /// `too_many_lines` clippy limit.
    pub(super) fn show_pane_menu(&self, ui: &mut egui::Ui, win: &mut PerWindowState) {
        if ui
            .add(self.menu_button_for("Split Vertical (Left | Right)", KeyAction::SplitVertical))
            .clicked()
        {
            self.spawn_split_pane(win, super::panes::SplitDirection::Horizontal);
            ui.close();
        }
        if ui
            .add(self.menu_button_for(
                "Split Horizontal (Top / Bottom)",
                KeyAction::SplitHorizontal,
            ))
            .clicked()
        {
            self.spawn_split_pane(win, super::panes::SplitDirection::Vertical);
            ui.close();
        }

        ui.separator();

        let can_close_pane = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(
                can_close_pane,
                self.menu_button_for("Close Pane", KeyAction::ClosePane),
            )
            .clicked()
        {
            win.pending_close_pane = true;
            ui.close();
        }

        let is_zoomed = win.tabs.active_tab().zoomed_pane.is_some();
        let zoom_label = if is_zoomed {
            "Un-Zoom Pane"
        } else {
            "Zoom Pane"
        };
        let can_zoom = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

        if ui
            .add_enabled(
                can_zoom,
                self.menu_button_for(zoom_label, KeyAction::ZoomPane),
            )
            .clicked()
        {
            let tab = win.tabs.active_tab_mut();
            let current = tab.active_pane;
            if tab.zoomed_pane == Some(current) {
                tab.zoomed_pane = None;
            } else {
                tab.zoomed_pane = Some(current);
            }
            ui.close();
        }
    }

    /// Render the tab bar between the menu bar and the terminal area.
    ///
    /// Shows one button per open tab (active tab visually distinguished
    /// with a colored underline), a close button (x) on each tab when
    /// more than one tab is open, and a "+" button at the end to create
    /// new tabs. Tabs are separated by thin vertical dividers.
    ///
    /// When `win.renaming_tab` matches a tab, that tab's label is replaced
    /// with a `TextEdit` bound to `win.rename_buffer`.  Enter commits,
    /// Escape cancels, and loss of focus also cancels (matches most
    /// file-manager rename UX).
    ///
    /// Returns a `TabBarAction` describing what the user did (if anything).
    pub(super) fn show_tab_bar(&self, win: &mut PerWindowState, ui: &mut egui::Ui) -> TabBarAction {
        // Escape cancels an in-progress drag without dispatching a reorder.
        // Checked before rendering so the dim/preview disappears on the same
        // frame the user presses Escape.
        if win.dragging_tab.is_some() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            win.dragging_tab = None;
        }

        // Compute the render order for this frame. When a tab is being
        // dragged, the source tab "floats" to the slot nearest the current
        // pointer position so the user sees a live preview of the reorder.
        // When no drag is active, render in natural order.
        //
        // The preview uses last frame's tab rects (stable between frames
        // unless the window resizes) because rects for the current frame
        // aren't known until AFTER rendering. One frame of lag at the start
        // of a drag is imperceptible.
        let count = win.tabs.tab_count();
        let preview_order: Vec<usize> = if let Some(from) = win.dragging_tab
            && !win.last_tab_rects.is_empty()
            && let Some(pointer) = ui.input(|i| i.pointer.latest_pos())
        {
            // Find the target insertion slot based on pointer.x vs each
            // tab's center. Walk from left to right: first tab whose center
            // is to the right of the pointer determines the slot.
            let mut target_slot = count; // default: drop at end
            for (i, rect) in win.last_tab_rects.iter().enumerate() {
                if pointer.x < rect.center().x {
                    target_slot = i;
                    break;
                }
            }

            // Build the preview order: original indices in order, but with
            // `from` extracted and reinserted at `target_slot`. Clamp the
            // slot to a valid insertion index after removal.
            let mut order: Vec<usize> = (0..count).filter(|&i| i != from).collect();
            let insert_at = target_slot.min(order.len());
            order.insert(insert_at, from);
            order
        } else {
            (0..count).collect()
        };

        // Render position of the dragged tab (index into `preview_order`),
        // used on `drag_stopped` to decide whether the reorder was a no-op.
        // Captured before rendering because `show_single_tab` clears
        // `dragging_tab` on `drag_stopped`.
        let drag_source: Option<usize> = win.dragging_tab;
        let source_render_pos =
            drag_source.and_then(|from| preview_order.iter().position(|&i| i == from));

        ui.horizontal(|ui| {
            let active = win.tabs.active_index();
            let renaming = win.renaming_tab;
            let mut action = TabBarAction::None;
            let mut current_rects: Vec<egui::Rect> = vec![egui::Rect::NOTHING; count];
            let mut drag_ended_this_frame = false;

            // Render tabs in the preview order. The source tab (while being
            // dragged) renders at its preview slot, dimmed; all other tabs
            // appear at their shifted positions so the user sees where the
            // drop will land.
            for (render_pos, &orig_idx) in preview_order.iter().enumerate() {
                if render_pos > 0 {
                    ui.separator();
                }

                let Some(tab) = win.tabs.iter().nth(orig_idx) else {
                    continue;
                };

                let is_echo_off = self.config.security.password_indicator
                    && tab
                        .active_pane()
                        .is_some_and(|p| p.echo_off.load(std::sync::atomic::Ordering::Relaxed));

                let is_renaming = renaming == Some(tab.id);
                let is_being_dragged = win.dragging_tab == Some(orig_idx);

                let (tab_action, tab_rect) = Self::show_single_tab(
                    ui,
                    tab,
                    orig_idx,
                    orig_idx == active,
                    count,
                    is_echo_off,
                    is_renaming,
                    is_being_dragged,
                    &mut win.rename_buffer,
                    &mut win.dragging_tab,
                    &mut drag_ended_this_frame,
                    self.config.tab_title.policy,
                    &self.config.tab_title.separator,
                );
                current_rects[orig_idx] = tab_rect;
                if !matches!(tab_action, TabBarAction::None) {
                    action = tab_action;
                }
            }

            ui.separator();

            // "+" button to create a new tab.
            if ui.button("+").clicked() {
                action = TabBarAction::NewTab;
            }

            // On drag release: dispatch Reorder if the preview position
            // differs from the source tab's original position. The source
            // `from` was captured at top-of-frame (before `show_single_tab`
            // cleared `dragging_tab`); `to` is the preview position we
            // rendered the source at this frame.
            if drag_ended_this_frame
                && let Some(from) = drag_source
                && let Some(to) = source_render_pos
                && from != to
            {
                action = TabBarAction::Reorder { from, to };
            }

            // Stash rects for next frame's preview computation — but ONLY
            // when this frame rendered in natural order (no drag was active
            // at the start of the frame). During a drag, `last_tab_rects`
            // must stay frozen at the natural pre-drag layout so
            // slot-decision boundaries don't move under the pointer.
            // Refreshing mid-drag causes oscillation with differently-sized
            // tabs: the ghost shifts a rect under the pointer, which flips
            // the decision, which shifts the rect back.
            //
            // We key off `drag_source` (captured at top-of-frame) rather
            // than `win.dragging_tab` (which may have been set mid-render
            // by `show_single_tab` on the drag-start frame). This ensures
            // the drag-start frame's natural-order rects are captured for
            // the drag to use.
            if drag_source.is_none() {
                win.last_tab_rects = current_rects;
            }

            action
        })
        .inner
    }

    /// Build the [`egui::Frame`] for a single tab.
    ///
    /// The frame owns the tab's entire visual (fill, border, corner radius) and
    /// — critically — has **constant geometry across every state**. Every tab,
    /// whether active, inactive, hovered, or dragged, uses the same
    /// `inner_margin` and the same `stroke` width, so hovering or activating a
    /// tab never changes its size and therefore never reflows the tab bar (or
    /// the terminal buffer below it). Only the *colors* change between states.
    ///
    /// Colors come from the active theme's [`ChromeRole`] palette so the tab
    /// bar follows the selected theme + profile — no hard-coded hues:
    ///
    /// - **active**: the saturated `Accent` fill with an `Accent`-colored
    ///   border, so the focused tab pops.
    /// - **hovered (inactive)**: the `SurfaceHover` fill with a `Border` stroke.
    /// - **inactive**: the `SurfaceVariant` fill with a `Border` stroke, so
    ///   non-active tabs always carry a visible outline in every theme.
    /// - **being-dragged**: a translucent ghost over the inactive look.
    /// - **bell-active (inactive)**: a warm tint blended toward the palette
    ///   warning color so an unacked bell reads as attention-worthy.
    // The four bools each describe an independent, orthogonal tab state
    // (active / hovered / being-dragged / bell). They are not a closed set of
    // mutually-exclusive variants, so an enum would not model them.
    #[allow(clippy::fn_params_excessive_bools)]
    fn tab_frame(
        ui: &egui::Ui,
        is_active: bool,
        is_hovered: bool,
        is_being_dragged: bool,
        has_bell: bool,
    ) -> egui::Frame {
        let v = ui.visuals();
        let corner = v.menu_corner_radius;
        let stroke_width = v.widgets.inactive.bg_stroke.width.max(1.0);

        // Fill + border colors per state, read from the themed `Visuals`
        // (built from the active palette's `ChromeRole`s in `chrome_style.rs`):
        //   Accent          = selection.bg_fill
        //   OnAccent border = selection.stroke.color
        //   SurfaceVariant  = widgets.inactive.bg_fill
        //   SurfaceHover    = widgets.hovered.bg_fill
        //   Border          = widgets.inactive.bg_stroke.color
        // Geometry is identical in all cases — only colors change.
        let (fill, border) = if is_active {
            (v.selection.bg_fill, v.selection.stroke.color)
        } else if is_hovered {
            (
                v.widgets.hovered.bg_fill,
                v.widgets.inactive.bg_stroke.color,
            )
        } else {
            (
                v.widgets.inactive.bg_fill,
                v.widgets.inactive.bg_stroke.color,
            )
        };
        let warn = v.warn_fg_color;

        // Blend toward the warning color for an unacked bell on an inactive tab.
        let fill = if has_bell && !is_active {
            egui::Color32::from_rgb(
                u8::midpoint(fill.r(), warn.r()),
                u8::midpoint(fill.g(), warn.g()),
                u8::midpoint(fill.b(), warn.b()),
            )
        } else {
            fill
        };

        // Dragged tabs render as a translucent ghost so the pickup is visible.
        let fill = if is_being_dragged {
            egui::Color32::from_rgba_unmultiplied(fill.r(), fill.g(), fill.b(), 96)
        } else {
            fill
        };

        egui::Frame::NONE
            .fill(fill)
            .stroke(egui::Stroke::new(stroke_width, border))
            .corner_radius(corner)
            // Uniform padding for every state keeps the tab a constant height,
            // so the label is vertically centered and hover never reflows.
            .inner_margin(egui::Margin::symmetric(8, 4))
    }

    /// Render the inline rename editor for a tab and return the resulting
    /// [`TabBarAction`].  Commit on Enter, cancel on Escape or focus loss.
    fn show_tab_rename_editor(
        ui: &mut egui::Ui,
        tab_id: super::tabs::TabId,
        index: usize,
        rename_buffer: &mut String,
    ) -> TabBarAction {
        let edit_id = egui::Id::new(("tab_rename", tab_id));
        let edit = egui::TextEdit::singleline(rename_buffer)
            .id(edit_id)
            .desired_width(120.0);
        let response = ui.add(edit);
        // Auto-focus on the first frame the editor appears.
        if !response.has_focus() && !response.lost_focus() {
            response.request_focus();
        }
        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
        if enter {
            TabBarAction::CommitRename(index, rename_buffer.clone())
        } else if escape || response.lost_focus() {
            TabBarAction::CancelRename
        } else {
            TabBarAction::None
        }
    }

    /// Render a single tab element with label, optional close button,
    /// and a distinct background color for the active tab.
    ///
    /// Inactive tabs with an unacknowledged bell are drawn with an amber
    /// text color and a warm-tinted background to make them more prominent.
    ///
    /// A lock icon is prepended to the label when `is_echo_off` is `true`,
    /// indicating that the foreground process has disabled terminal echo (i.e.
    /// a password prompt such as `sudo` or `ssh` is waiting for input).
    ///
    /// When `is_renaming` is `true`, the label is replaced by a `TextEdit`
    /// bound to `rename_buffer`.  Enter returns `CommitRename`; Escape
    /// returns `CancelRename`; losing focus also cancels to avoid leaving
    /// the window in a stuck-edit state.
    ///
    /// A double-click on the label returns `BeginRename(index)`.
    ///
    /// A mouse drag on the label sets `*dragging_tab = Some(index)`; on
    /// release, `*drag_ended_this_frame = true` signals the caller to
    /// dispatch the reorder action. The caller (`show_tab_bar`) owns the
    /// preview-order logic and decides the final destination slot.
    /// While `is_being_dragged` is true, the tab is rendered with a
    /// distinct fill so the user can see which tab they picked up.
    ///
    /// Returns the tab action (if any) plus the screen rect of the label
    /// area so the caller can hit-test drop targets.
    // Allows:
    // - too_many_arguments / fn_params_excessive_bools: this is a single-use
    //   helper for `show_tab_bar`. Each bool (is_active, is_echo_off,
    //   is_renaming, is_being_dragged) reflects an independent per-tab UI
    //   state; folding them into an enum would require a 16-variant combo
    //   or nested structs that add more noise than the current flat list.
    #[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
    pub(super) fn show_single_tab(
        ui: &mut egui::Ui,
        tab: &Tab,
        index: usize,
        is_active: bool,
        count: usize,
        is_echo_off: bool,
        is_renaming: bool,
        is_being_dragged: bool,
        rename_buffer: &mut String,
        dragging_tab: &mut Option<usize>,
        drag_ended_this_frame: &mut bool,
        title_policy: TabTitlePolicy,
        title_separator: &str,
    ) -> (TabBarAction, egui::Rect) {
        let mut action = TabBarAction::None;
        let pane = tab.active_pane();
        // Resolve the tab label under the configured title policy, combining
        // the user-assigned custom name with the shell-asserted OSC title.
        // An empty result falls back to a "Shell" placeholder.
        let resolved = tab.display_name(title_policy, title_separator);
        let label = if resolved.is_empty() {
            "Shell"
        } else {
            resolved.as_ref()
        };

        let has_bell = pane.is_some_and(|p| p.bell_active) && !is_active;

        // The tab label text itself; status indicators (broadcast, lock, bell)
        // are drawn as separate bundled-icon glyphs ahead of it (see the icon
        // prefixes rendered below), not embedded in this string — the label uses
        // the proportional UI font, which carries no Nerd Font icon glyphs.
        let display_label = label.to_owned();

        // Two-phase frame: lay the content out first (so we know the tab's
        // hover state and final size), then style + paint the frame around it.
        // This keeps the tab geometry constant across active/inactive/hover
        // states — the frame's fill/stroke change but its margins never do —
        // so hovering or activating a tab never reflows the tab bar (bug: tab
        // bar grew on hover) and the label stays vertically centered.
        let mut prepared = egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(8, 4))
            .begin(ui);

        let mut tab_hovered = false;
        {
            let ui = &mut prepared.content_ui;
            // Center the row vertically within the constant-height frame so the
            // label sits in the middle regardless of active/inactive state.
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if is_renaming {
                    action = Self::show_tab_rename_editor(ui, tab.id, index, rename_buffer);
                } else {
                    // Status indicators, drawn as bundled icon glyphs ahead
                    // of the label (palette-tinted, monospace family so they
                    // resolve from the bundled Nerd Font rather than the
                    // proportional UI font). Order: broadcast, lock, bell.
                    let warn_color = ui.visuals().warn_fg_color;
                    // Text color: legible OnAccent over the active tab's
                    // accent fill; otherwise the normal/bell color.
                    let label_color = if is_active {
                        ui.visuals().selection.stroke.color
                    } else if has_bell {
                        warn_color
                    } else {
                        ui.visuals().widgets.inactive.fg_stroke.color
                    };
                    if tab.broadcast_input {
                        // Broadcast indicator (Task 74): tab is broadcasting
                        // keyboard input to all its panes.
                        ui.label(ChromeIcon::Broadcast.rich_text_colored(label_color));
                    }
                    if is_echo_off {
                        // Echo disabled (password prompt active).
                        ui.label(ChromeIcon::LockKey.rich_text_colored(warn_color));
                    }
                    if has_bell {
                        // Unacknowledged bell on a non-focused tab.
                        ui.label(ChromeIcon::Bell.rich_text_colored(warn_color));
                    }

                    let rich_label = egui::RichText::new(&display_label)
                        .size(13.0)
                        .color(label_color);

                    // Frameless button: the tab frame owns the visual, so
                    // the button must not draw its own background/border
                    // (that is what made non-active tabs borderless and grew
                    // the bar on hover). It still senses click_and_drag so
                    // the tab can be picked up and dragged.
                    let response = ui.add(
                        egui::Button::new(rich_label)
                            .frame(false)
                            .sense(egui::Sense::click_and_drag()),
                    );
                    if response.hovered() {
                        tab_hovered = true;
                    }
                    if response.double_clicked() {
                        action = TabBarAction::BeginRename(index);
                    } else if response.clicked() && !is_active {
                        action = TabBarAction::SwitchTo(index);
                    }
                    if response.drag_started() {
                        *dragging_tab = Some(index);
                    }
                    if response.drag_stopped() {
                        // Signal the caller that a drag ended this frame.
                        // The actual drop resolution (source vs. preview
                        // position) is handled in `show_tab_bar` using the
                        // preview-order state captured at top of frame.
                        *drag_ended_this_frame = true;
                        *dragging_tab = None;
                    }

                    // Right-click context menu. "Clear Custom Name" is only
                    // offered when the tab has a user-assigned custom name.
                    response.context_menu(|ui| {
                        if ui.button("Rename Tab\u{2026}").clicked() {
                            action = TabBarAction::BeginRename(index);
                            ui.close();
                        }
                        if tab.custom_name.is_some() && ui.button("Clear Custom Name").clicked() {
                            action = TabBarAction::ClearCustomName(index);
                            ui.close();
                        }
                    });

                    // Show close button when more than one tab is open.
                    if count > 1 && ui.small_button(ChromeIcon::Close.rich_text()).clicked() {
                        action = TabBarAction::Close(index);
                    }
                }
            });
        }

        // Whole-tab hover (covers the padding around the label), so the hover
        // highlight tracks the entire tab rect, not just the label glyphs.
        let outer_rect = prepared
            .content_ui
            .min_rect()
            .expand2(egui::Vec2::new(8.0, 4.0));
        if ui.rect_contains_pointer(outer_rect) {
            tab_hovered = true;
        }

        // Now that the content is laid out and hover is known, apply the
        // state-dependent fill/stroke and paint the frame behind the content.
        prepared.frame = Self::tab_frame(ui, is_active, tab_hovered, is_being_dragged, has_bell);
        prepared.paint(ui);
        let frame_response = prepared.allocate_space(ui);

        (action, frame_response.rect)
    }

    /// Show the floating "Save Layout" name-entry prompt.
    ///
    /// Rendered every frame when `pending_save_layout` is `Some`.  Returns
    /// `true` exactly once — on the frame when the user confirms the save —
    /// which the caller should use to dispatch `KeyAction::SaveLayout`.
    ///
    /// The prompt is dismissed (setting `pending_save_layout` back to `None`)
    /// on both Save and Cancel.
    pub(super) fn show_save_layout_prompt(&mut self, ctx: &egui::Context) -> bool {
        if self.pending_save_layout.is_none() {
            return false;
        }

        let mut confirmed = false;
        let mut cancelled = false;

        egui::Window::new("Save Layout")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label("Enter a name for this layout:");
                ui.add_space(4.0);

                let mut name_buf = self.pending_save_layout.clone().unwrap_or_default();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut name_buf)
                        .hint_text("e.g. dev, work, personal")
                        .desired_width(240.0),
                );
                // Request focus only on the first frame so the user can start
                // typing immediately, but clicking elsewhere (e.g. the Save or
                // Cancel button) is not overridden on subsequent frames.
                if self.save_layout_prompt_just_opened {
                    response.request_focus();
                    self.save_layout_prompt_just_opened = false;
                }
                self.pending_save_layout = Some(name_buf.clone());

                let can_save = !name_buf.is_empty();
                // Confirm on Enter (whether focus was lost by Enter or not).
                let enter_pressed = ui.input(|i| i.key_pressed(egui::Key::Enter));

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(can_save, egui::Button::new("Save"))
                        .clicked()
                        || (enter_pressed && can_save)
                    {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked()
                        || ui.input(|i| i.key_pressed(egui::Key::Escape))
                    {
                        cancelled = true;
                    }
                });
            });

        if confirmed || cancelled {
            self.pending_save_layout = if confirmed {
                // Leave the name in place so `dispatch_deferred_action` can
                // read it via `pending_save_layout.take()`.
                self.pending_save_layout.clone()
            } else {
                None
            };
        }

        confirmed
    }
}
