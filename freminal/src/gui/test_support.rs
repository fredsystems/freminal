// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Test fixtures for GUI code that needs real tabs, panes and a
//! [`FreminalGui`]: panes whose `input_tx` is observable, windows made of
//! them, and an app holding those windows.
//!
//! None of it spawns a PTY or a notification service.

use std::sync::{Arc, Mutex, OnceLock, atomic::AtomicBool};

use arc_swap::ArcSwap;
use clap::Parser;
use crossbeam_channel::Receiver;
use freminal_common::args::Args;
use freminal_common::config::Config;
use freminal_terminal_emulator::io::{InputEvent, SearchCorpus};
use freminal_terminal_emulator::snapshot::TerminalSnapshot;
use freminal_windowing::WindowId;

use super::panes::{Pane, PaneId, PaneIdGenerator};
use super::pty::WindowCommandBudget;
use super::renderer::WindowPostRenderer;
use super::tabs::{Tab, TabId, TabManager};
use super::terminal::FreminalTerminalWidget;
use super::title_stack::TitleStack;
use super::view_state::ViewState;
use super::{FreminalGui, StartupState};

/// A tab with one pane whose input receiver is returned.
pub(super) fn tab_with_input_rx(id: TabId, pane_id: PaneId) -> (Tab, Receiver<InputEvent>) {
    let arc_swap = Arc::new(ArcSwap::from_pointee(TerminalSnapshot::empty()));
    let (input_tx, input_rx) = crossbeam_channel::unbounded();
    let (_window_cmd_tx, window_cmd_rx) = crossbeam_channel::unbounded();
    let (_clipboard_tx, clipboard_rx) = crossbeam_channel::bounded(1);
    let (_search_buffer_tx, search_buffer_rx) = crossbeam_channel::bounded::<SearchCorpus>(1);
    let (_pty_dead_tx, pty_dead_rx) = crossbeam_channel::bounded(1);
    let (_command_event_tx, command_event_rx) = crossbeam_channel::unbounded();

    let pane = Pane {
        id: pane_id,
        arc_swap,
        reply_tx: Arc::new(input_tx.clone()),
        input_tx,
        window_cmd_rx,
        clipboard_rx,
        search_buffer_rx,
        pty_dead_rx,
        command_event_rx,
        recent_commands: std::collections::VecDeque::new(),
        history_seed: crate::gui::shell_history::new_seeded_history(),
        shell_program: None,
        shell_histfile_last_seen: None,
        command_texts: std::collections::HashMap::new(),
        title: format!("pane-{pane_id}"),
        bell_active: false,
        pending_copy: false,
        title_stack: TitleStack::new(),
        view_state: ViewState::new(),
        echo_off: Arc::new(AtomicBool::new(false)),
        window_cmd_budget: WindowCommandBudget::default(),
        child_pid: None,
        render_state: crate::gui::terminal::new_render_state(Arc::new(std::sync::Mutex::new(
            crate::gui::renderer::WindowPostRenderer::new(),
        ))),
        render_cache: crate::gui::terminal::PaneRenderCache::new(),
    };

    (Tab::new(id, pane), input_rx)
}

/// A window of `count` tabs (one pane each) and the panes' receivers.
pub(super) fn window_of(
    ids: &mut crate::gui::panes::PaneIdGenerator,
    count: u64,
) -> (TabManager, Vec<Receiver<InputEvent>>) {
    let (first, rx) = tab_with_input_rx(TabId::first(), ids.next_id());
    let mut receivers = vec![rx];
    let mut tabs = TabManager::new(first);
    for n in 1..count {
        let (tab, rx) = tab_with_input_rx(TabId::offset(n), ids.next_id());
        tabs.add_tab(tab);
        receivers.push(rx);
    }
    (tabs, receivers)
}

/// One window's panes: `(pane id, receiver of its input channel)` per pane,
/// in tab order.
pub(super) type WindowPanes = Vec<(PaneId, Receiver<InputEvent>)>;

/// A [`FreminalGui`] built from `config` holding one window per entry of
/// `tab_counts`, each with that many tabs of one pane. Returns the app and,
/// per window, its panes' `(id, input receiver)` pairs in tab order.
///
/// Window `n` (counting from 1) has the id `WindowId::for_test(n)`.
pub(super) fn gui_with_windows(
    config: &Config,
    tab_counts: &[u64],
) -> (FreminalGui, Vec<WindowPanes>) {
    let window_post = Arc::new(Mutex::new(WindowPostRenderer::new()));
    let repaint_handle = Arc::new(OnceLock::new());
    // Empty injected state: the fixture must not read the developer's
    // persisted state or layout library.
    let mut gui = FreminalGui::with_startup_state(
        config.clone(),
        Args::parse_from(["freminal"]),
        Arc::clone(&repaint_handle),
        None,
        Arc::clone(&window_post),
        Arc::new(arc_swap::ArcSwapOption::empty()),
        StartupState::default(),
    );
    let ctx = egui::Context::default();
    let mut ids = PaneIdGenerator::new(0);
    let mut panes = Vec::new();
    for (n, &count) in (1_u64..).zip(tab_counts) {
        let widget = FreminalTerminalWidget::new(&ctx, config)
            .unwrap_or_else(|e| panic!("terminal widget: {e}"));
        let (first, first_rx) = tab_with_input_rx(TabId::first(), ids.next_id());
        let mut window = FreminalGui::new_per_window_state(
            first,
            widget,
            false,
            Arc::clone(&window_post),
            Arc::clone(&repaint_handle),
        );
        let mut receivers = vec![first_rx];
        for k in 1..count {
            let (tab, rx) = tab_with_input_rx(TabId::offset(k), ids.next_id());
            window.tabs.add_tab(tab);
            receivers.push(rx);
        }
        let mut pane_ids = Vec::new();
        for tab in &window.tabs {
            if let Ok(tab_panes) = tab.pane_tree.iter_panes() {
                pane_ids.extend(tab_panes.into_iter().map(|p| p.id));
            }
        }
        gui.windows.insert(WindowId::for_test(n), window);
        panes.push(pane_ids.into_iter().zip(receivers).collect());
    }
    (gui, panes)
}
