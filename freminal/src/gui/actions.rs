// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use eframe::egui;
use freminal_terminal_emulator::io::InputEvent;
use tracing::{error, trace};

impl super::FreminalGui {
    /// Close the root window, or show a confirmation dialog if secondary
    /// windows are still open.
    ///
    /// When no secondary windows exist, sends `ViewportCommand::Close` to
    /// terminate the app immediately.  Otherwise sets the
    /// `show_close_confirmation` flag which causes `ui()` to render a
    /// modal dialog asking "Close all windows?".
    pub(super) fn close_or_hide_root(&mut self, ctx: &egui::Context) {
        if self.secondary_windows.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            self.show_close_confirmation = true;
        }
    }

    pub(super) fn close_focused_pane(&mut self, ui: &egui::Ui) {
        let tab = self.tabs.active_tab_mut();
        let target = tab.active_pane;

        if tab.zoomed_pane == Some(target) {
            tab.zoomed_pane = None;
        }

        match tab.pane_tree.close(target) {
            Ok(_closed) => {
                let tab = self.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        pane.view_state.last_sent_size = (0, 0);
                    }
                }
                let tab = self.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes()
                    && let Some(first) = panes.first()
                {
                    let new_id = first.id;
                    if let Err(e) = first.input_tx.send(InputEvent::FocusChange(true)) {
                        error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
                    }
                    tab.active_pane = new_id;
                }
            }
            Err(super::panes::PaneError::CannotCloseLastPane) => {
                if self.tabs.tab_count() <= 1 {
                    self.close_or_hide_root(ui.ctx());
                    return;
                }
                let idx = self.tabs.active_index();
                self.close_tab(idx);
            }
            Err(e) => {
                error!("Failed to close pane {target}: {e}");
            }
        }
    }

    pub(super) fn focus_pane_in_direction(
        &mut self,
        direction: freminal_common::keybindings::KeyAction,
        available_rect: egui::Rect,
    ) {
        use freminal_common::keybindings::KeyAction;

        let tab = self.tabs.active_tab();

        if tab.zoomed_pane.is_some() {
            return;
        }

        let current_id = tab.active_pane;

        let layout = match tab.pane_tree.layout(available_rect) {
            Ok(l) => l,
            Err(e) => {
                error!("Failed to compute pane layout for navigation: {e}");
                return;
            }
        };

        let Some(current_rect) = layout
            .iter()
            .find(|(id, _)| *id == current_id)
            .map(|(_, r)| *r)
        else {
            return;
        };
        let current_center = current_rect.center();

        let best = layout
            .iter()
            .filter(|(id, _)| *id != current_id)
            .filter(|(_, rect)| {
                let c = rect.center();
                match direction {
                    KeyAction::FocusPaneLeft => c.x < current_center.x,
                    KeyAction::FocusPaneRight => c.x > current_center.x,
                    KeyAction::FocusPaneUp => c.y < current_center.y,
                    KeyAction::FocusPaneDown => c.y > current_center.y,
                    _ => false,
                }
            })
            .min_by(|(_, a), (_, b)| {
                let dist_a = a.center().distance(current_center);
                let dist_b = b.center().distance(current_center);
                dist_a
                    .partial_cmp(&dist_b)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });

        if let Some((new_id, _)) = best {
            let new_id = *new_id;
            let tab = self.tabs.active_tab_mut();
            let old_id = tab.active_pane;

            if let Some(old_pane) = tab.pane_tree.find(old_id)
                && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
            {
                error!("Failed to send FocusChange(false) to pane {old_id}: {e}");
            }

            tab.active_pane = new_id;

            if let Some(new_pane) = tab.pane_tree.find(new_id)
                && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
            {
                error!("Failed to send FocusChange(true) to pane {new_id}: {e}");
            }
        }
    }

    pub(super) fn dispatch_tab_bar_action(&mut self, action: super::TabBarAction) {
        match action {
            super::TabBarAction::NewTab => self.spawn_new_tab(),
            super::TabBarAction::SwitchTo(i) => {
                if let Err(e) = self.tabs.switch_to(i) {
                    error!("Failed to switch tab: {e}");
                } else {
                    self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
                }
            }
            super::TabBarAction::Close(i) => self.close_tab(i),
            super::TabBarAction::None => {}
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn dispatch_deferred_action(
        &mut self,
        action: freminal_common::keybindings::KeyAction,
    ) {
        use freminal_common::keybindings::KeyAction;

        match action {
            KeyAction::OpenSettings => {
                if !self.settings_modal.is_open {
                    let families = self.terminal_widget.monospace_families();
                    self.settings_modal
                        .open(&self.config, families, self.os_dark_mode);
                    self.settings_modal
                        .set_base_font_defs(self.terminal_widget.base_font_defs().clone());
                }
            }
            KeyAction::NewTab => self.spawn_new_tab(),
            KeyAction::CloseTab => {
                if let Err(e) = self.tabs.close_active_tab() {
                    trace!("Cannot close tab: {e}");
                }
            }
            KeyAction::NextTab => {
                self.tabs.next_tab();
                self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
            }
            KeyAction::PrevTab => {
                self.tabs.prev_tab();
                self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
            }
            KeyAction::SwitchToTab1 => self.switch_to_tab_n(0),
            KeyAction::SwitchToTab2 => self.switch_to_tab_n(1),
            KeyAction::SwitchToTab3 => self.switch_to_tab_n(2),
            KeyAction::SwitchToTab4 => self.switch_to_tab_n(3),
            KeyAction::SwitchToTab5 => self.switch_to_tab_n(4),
            KeyAction::SwitchToTab6 => self.switch_to_tab_n(5),
            KeyAction::SwitchToTab7 => self.switch_to_tab_n(6),
            KeyAction::SwitchToTab8 => self.switch_to_tab_n(7),
            KeyAction::SwitchToTab9 => self.switch_to_tab_n(8),
            KeyAction::MoveTabLeft => self.tabs.move_active_left(),
            KeyAction::MoveTabRight => self.tabs.move_active_right(),
            KeyAction::ZoomIn => self.apply_zoom(1.0),
            KeyAction::ZoomOut => self.apply_zoom(-1.0),
            KeyAction::ZoomReset => {
                self.tabs
                    .active_tab_mut()
                    .active_pane_mut()
                    .view_state
                    .reset_zoom();
                self.terminal_widget.apply_font_zoom(self.config.font.size);
                self.invalidate_all_pane_atlases();
            }
            KeyAction::OpenSearch => {
                self.tabs
                    .active_tab_mut()
                    .active_pane_mut()
                    .view_state
                    .search_state
                    .is_open = true;
            }
            KeyAction::SearchNext => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                pane.view_state.search_state.next_match();
                let snap = pane.arc_swap.load();
                super::search::scroll_to_match_and_send(
                    &mut pane.view_state,
                    &snap,
                    &pane.input_tx,
                );
            }
            KeyAction::SearchPrev => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                pane.view_state.search_state.prev_match();
                let snap = pane.arc_swap.load();
                super::search::scroll_to_match_and_send(
                    &mut pane.view_state,
                    &snap,
                    &pane.input_tx,
                );
            }
            KeyAction::PrevCommand => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                let snap = pane.arc_swap.load();
                super::search::jump_to_prev_command(&mut pane.view_state, &snap);
            }
            KeyAction::NextCommand => {
                let tab = self.tabs.active_tab_mut();
                let pane = tab.active_pane_mut();
                let snap = pane.arc_swap.load();
                super::search::jump_to_next_command(&mut pane.view_state, &snap);
            }
            KeyAction::NewWindow => {
                self.pending_new_window = true;
            }
            KeyAction::RenameTab => {
                trace!("Unhandled deferred key action: {action:?}");
            }
            KeyAction::SplitVertical => {
                self.spawn_split_pane(super::panes::SplitDirection::Horizontal);
            }
            KeyAction::SplitHorizontal => {
                self.spawn_split_pane(super::panes::SplitDirection::Vertical);
            }
            KeyAction::ClosePane => {
                self.pending_close_pane = true;
            }
            KeyAction::FocusPaneLeft
            | KeyAction::FocusPaneDown
            | KeyAction::FocusPaneUp
            | KeyAction::FocusPaneRight => {
                self.pending_focus_direction = Some(action);
            }
            KeyAction::ResizePaneLeft => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    super::panes::SplitDirection::Horizontal,
                    -0.05,
                ) {
                    trace!("Cannot resize pane left: {e}");
                }
            }
            KeyAction::ResizePaneRight => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    super::panes::SplitDirection::Horizontal,
                    0.05,
                ) {
                    trace!("Cannot resize pane right: {e}");
                }
            }
            KeyAction::ResizePaneUp => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    super::panes::SplitDirection::Vertical,
                    -0.05,
                ) {
                    trace!("Cannot resize pane up: {e}");
                }
            }
            KeyAction::ResizePaneDown => {
                let id = self.tabs.active_tab().active_pane;
                if let Err(e) = self.tabs.active_tab_mut().pane_tree.resize_split(
                    id,
                    super::panes::SplitDirection::Vertical,
                    0.05,
                ) {
                    trace!("Cannot resize pane down: {e}");
                }
            }
            KeyAction::ZoomPane => {
                let tab = self.tabs.active_tab_mut();
                let current = tab.active_pane;
                if tab.zoomed_pane == Some(current) {
                    tab.zoomed_pane = None;
                } else {
                    tab.zoomed_pane = Some(current);
                }
                let tab = self.tabs.active_tab_mut();
                if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                    for pane in panes {
                        pane.view_state.last_sent_size = (0, 0);
                    }
                }
            }
            KeyAction::Copy
            | KeyAction::Paste
            | KeyAction::SelectAll
            | KeyAction::ToggleMenuBar
            | KeyAction::ScrollPageUp
            | KeyAction::ScrollPageDown
            | KeyAction::ScrollToTop
            | KeyAction::ScrollToBottom
            | KeyAction::ScrollLineUp
            | KeyAction::ScrollLineDown => {
                trace!(
                    "Unexpected deferred key action (should be handled at input layer): {action:?}"
                );
            }
        }
    }

    pub(super) fn apply_zoom(&mut self, delta: f32) {
        let base = self.config.font.size;
        let vs = &mut self.tabs.active_tab_mut().active_pane_mut().view_state;
        vs.adjust_zoom(base, delta);
        let effective = vs.effective_font_size(base);
        self.terminal_widget.apply_font_zoom(effective);
        self.invalidate_all_pane_atlases();
    }

    pub(super) fn invalidate_all_pane_atlases(&mut self) {
        for tab in self.tabs.iter_mut() {
            if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                for pane in panes {
                    pane.render_state
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clear_atlas();
                    pane.render_cache.invalidate_content();
                }
            }
        }
    }

    pub(super) fn switch_to_tab_n(&mut self, index: usize) {
        if let Err(e) = self.tabs.switch_to(index) {
            trace!("Cannot switch to tab {index}: {e}");
        } else {
            self.tabs.active_tab_mut().active_pane_mut().bell_active = false;
        }
    }

    pub(super) fn close_tab(&mut self, index: usize) {
        if let Err(e) = self.tabs.close_tab(index) {
            trace!("Cannot close tab: {e}");
        }
    }
}
