// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use conv2::{ApproxFrom, ConvUtil, ValueFrom};
use egui::{self, CentralPanel, Panel, ViewportCommand};
use egui_glow::CallbackFn;
use freminal_common::buffer_states::window_manipulation::Osc99ControlKind;
use freminal_common::config::ThemeMode;
use freminal_common::geometry::Rect;
use freminal_common::pty_write::PtyWrite;
use freminal_common::send_or_log;
use freminal_terminal_emulator::io::InputEvent;
use freminal_windowing::WindowId;
use tracing::{debug, error, trace, warn};

use super::chrome_damage;
use super::frame_damage;
use super::frame_drain::{
    DeadPaneOutcome, WindowFocus, WindowManipulationEvents, drain_command_finished_events,
    drain_window_manipulation_commands, process_dead_panes,
};
use super::geometry_interop;
use super::panes;
use super::pointer_motion::{
    PaneResolution, PaneSnapshotInputs, animation_in_flight_composed,
    pointer_motion_needs_repaint_decision, resolve_pane_under_pointer,
};
use super::renderer::{WindowPostRenderer, gl_facade::Gl};
use super::rendering;
use super::tabs::{Tab, TabManager};
use super::terminal::{FreminalTerminalWidget, SplitBorderHover};
use super::view_state;
use super::window::PerWindowState;
use super::{FreminalGui, PaneBorderDrag};

/// What `on_close_requested` should do about a window that may own an open
/// Settings window (issue #401).
///
/// Pure decision over an already-computed boolean (rather than `WindowId`s
/// or `&FreminalGui` directly) so it is unit-testable without constructing
/// the windowing layer or a full `FreminalGui`.
///
/// There is no "already resolved, proceed normally" variant: the
/// `WindowUnsavedSettings` Force Close handler clears `settings_owner` to
/// `None` *before* re-issuing the window's close, so `is_owner` is already
/// `false` by the time `on_close_requested` runs again for the retry —
/// tracking a separate "confirmed" flag would be dead weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsOwnerCloseDecision {
    /// This window does not own the settings modal — no special handling.
    NotOwner,
    /// No unsaved settings edits (or none open) — close the settings window
    /// now, alongside this window.
    CloseNow,
    /// Unsaved settings edits — veto this window's close and surface the
    /// close-guard confirmation dialog.
    VetoWithPrompt,
}

/// Decide what `on_close_requested` should do about the settings window when
/// `window_id` (the window being closed) may own it.
///
/// `is_owner` is `self.settings_owner == Some(window_id)`. `has_unsaved` is
/// `self.settings_modal.has_unsaved_changes()`.
const fn settings_owner_close_decision(
    is_owner: bool,
    has_unsaved: bool,
) -> SettingsOwnerCloseDecision {
    if !is_owner {
        return SettingsOwnerCloseDecision::NotOwner;
    }
    if has_unsaved {
        return SettingsOwnerCloseDecision::VetoWithPrompt;
    }
    SettingsOwnerCloseDecision::CloseNow
}

/// Whether a pane's blink-style cursor needs the periodic ~500ms repaint
/// wake this frame.
///
/// A blinking cursor only needs to be re-rendered on a timer while it is
/// actually visible on screen. The drawing side computes visibility as
/// `snap.show_cursor && !is_echo_off && is_active_pane`
/// (`terminal/widget.rs`'s `effective_show_cursor`); the repaint scheduler
/// MUST gate the blink wake on the same condition, or a full-screen TUI that
/// hides the cursor via DECTCEM (`\e[?25l`) — btop, htop, vim, less — keeps
/// the default blink *style* and makes the terminal wake ~2x/sec forever to
/// redraw an unchanged, cursor-hidden screen (a real idle-CPU drain).
///
/// Returns `true` only when the cursor style is a blink variant AND the
/// cursor is actually visible (DECTCEM-shown, this is the active pane, and
/// password-echo-off is not hiding it).
const fn cursor_blink_wants_repaint(
    style: &freminal_common::cursor::CursorVisualStyle,
    show_cursor: bool,
    is_active: bool,
    is_echo_off: bool,
) -> bool {
    let is_blink_style = matches!(
        style,
        freminal_common::cursor::CursorVisualStyle::BlockCursorBlink
            | freminal_common::cursor::CursorVisualStyle::UnderlineCursorBlink
            | freminal_common::cursor::CursorVisualStyle::VerticalLineCursorBlink,
    );
    is_blink_style && show_cursor && is_active && !is_echo_off
}

/// #459 item 9: whether pointer motion this frame must force a full present.
/// Motion only changes plain-egui-painter chrome pixels when over a
/// chrome-interactive region, or while a pane-border drag is latched (the
/// pointer may be off the sensor mid-drag). Pure so it is unit-testable.
const fn pointer_forces_full_present(
    pointer_moving: bool,
    pointer_over_chrome: bool,
    border_drag_active: bool,
) -> bool {
    pointer_moving && (pointer_over_chrome || border_drag_active)
}

/// Per-frame boolean signals [`stage_frame_damage`] needs from its caller,
/// computed earlier in `central_body` from state this function has no
/// access to (egui overlay bookkeeping, the previous-active-pane cache, the
/// border-drag latch). Grouped into a params struct to stay under clippy's
/// `too_many_arguments` threshold — [`rendering::WindowManipFlags`] is the
/// in-tree precedent for this pattern. Each field is an independent,
/// simultaneously-observable signal (per `freminal-state-representation`'s
/// bool-vs-enum rule this is the "independent simultaneous signals"
/// exemption, not a state machine), so grouping in a struct rather than
/// converting to enums is correct.
struct FrameDamageInputs {
    /// Whether a menu/settings/context-menu-class overlay is open this
    /// frame (forces `Full`).
    ui_overlay_open: bool,
    /// Whether the active pane changed this frame (the border highlight
    /// moved, forces `Full`).
    active_pane_changed: bool,
    /// Whether a pane-border resize drag is currently latched.
    border_drag_active: bool,
}

/// Observations [`stage_frame_damage`] computes alongside its
/// [`freminal_windowing::FrameDamage`] decision, needed by `central_body`
/// after that call returns: the Task 121 frame-profiling duty-cycle
/// counters (gated in the caller, not here — see that function's doc), the
/// #436 `ChromeSignals` staging (`central_body`, near
/// `publish_pending_chrome_signals`), and the frame-attribution diagnostic
/// block that immediately follows this frame's decision.
// struct_excessive_bools: each field is an independent yes/no observation
// this frame (window-post shader state, toast/overlay presence, per-pane
// overlay OR-accumulation) feeding two unrelated downstream decisions (the
// #436 `ChromeSignals` staging and the Task 121 duty-cycle counters) — not
// a state machine. `force_full`/`unresolved_pane` are additionally
// `#[cfg(feature = "frame-profiling")]`-gated (mirrors `FrameStats`'s own
// per-field gating in `window.rs`): they are read only by the caller's
// feature-gated counters, so a default build must not carry them at all.
#[allow(clippy::struct_excessive_bools)]
struct FrameDamageObservations {
    /// Whether a window-post shader is recompositing the whole window this
    /// frame. Reused as `ChromeSignals::shader_active`.
    shader_recomposites: bool,
    /// The `force_full` term as decided here (`ui_overlay_open ||
    /// shader_recomposites || active_pane_changed ||
    /// pointer_forces_full_present(..)`), read only by the caller's
    /// `#[cfg(feature = "frame-profiling")]` duty-cycle counters.
    #[cfg(feature = "frame-profiling")]
    force_full: bool,
    /// Whether a pane in `pane_layout` could not be resolved in the pane
    /// tree this frame (forces `Full`), read only by the caller's
    /// `#[cfg(feature = "frame-profiling")]` duty-cycle counters.
    #[cfg(feature = "frame-profiling")]
    unresolved_pane: bool,
    /// Whether a toast or the resize overlay is animating this frame.
    /// Reused as `ChromeSignals::toast_active` and by the caller's
    /// feature-gated duty-cycle counters.
    toast_active: bool,
    /// OR-accumulated across every rendered pane: whether any has an open
    /// overlay that paints above the terminal band. Reused as
    /// `ChromeSignals::foreground_overlay_open`.
    foreground_overlay_open: bool,
    /// The per-pane damage facts fed to `decide_frame_damage`, reused by the
    /// caller to compute `ChromeSignals::bell_active` and by its
    /// feature-gated duty-cycle counters.
    per_pane_damage: Vec<frame_damage::PaneDamageInput>,
    /// The active pane's per-frame damage class (`None` if the active pane
    /// could not be resolved this frame), reused by the frame-attribution
    /// diagnostic stats that follow in `central_body`.
    active_pane_damage: Option<crate::gui::renderer::PaneFrameDamage>,
}

/// Decide this frame's #435 [`freminal_windowing::FrameDamage`], extracted
/// from `App::update`'s `central_body` closure (Task 122.9).
///
/// ## Double-write contract — do not collapse
///
/// `win.pending_frame_damage` is written **twice** per frame, deliberately:
///
/// 1. **Here** (the caller assigns this function's return value) — the
///    PRE-composition value, decided from #435 signals only.
/// 2. **Again**, unconditionally, by `frame_damage::compose_with_chrome_damage`
///    after `central_body` returns (near the end of `update()`) — which can
///    *upgrade* a `Partial` decided here to `Full` once the #436 chrome-cache
///    decision is known. That decision is not available yet at this call
///    site: it needs the after-toast-render dismissible-presence sample,
///    which can only be taken once `central_body` returns.
///
/// This function returns the `FrameDamage` rather than assigning
/// `win.pending_frame_damage` itself, so write #1 stays visible at the call
/// site and is not confused with write #2. Merging the two writes into one
/// — or moving this decision to run after the composition step — silently
/// reintroduces the bug the second write exists to prevent (a `Partial`
/// frame presented while chrome pixels outside the cursor rect changed);
/// see `compose_with_chrome_damage`'s call site for the defect it fixed.
/// [`tests::pending_frame_damage_double_write_stays_distinct`] pins that the
/// pre- and post-composition values can differ, so a future collapse of
/// the two writes fails loudly.
///
/// ## `pointer_over_chrome` — do not use `is_chrome_interactive_at`
///
/// `win` was removed from `self.windows` at the top of `update()` for this
/// frame (see `self.windows.remove` there), so `FreminalGui::
/// is_chrome_interactive_at` would find no window for `window_id` and
/// always return `true`. This function has no access to `self` at all, so
/// that mistake cannot be made here structurally — but if `self` is ever
/// threaded into a future revision of this function, do not reach for that
/// method; hit-test `win`'s own published chrome rects directly, as below.
///
/// ## Feature-gated duty-cycle counters — NOT computed here
///
/// The Task 121 `chrome_mode` duty-cycle and `zero_change_presented`
/// counters read this function's outputs but are recorded by the caller
/// under its own `#[cfg(feature = "frame-profiling")]` block, not inside
/// this function (Subtask 122.5's contract: return what the counters need
/// and gate only the recording). `chrome_mode` is already a parameter of
/// `App::update` itself, so `central_body` has it in scope without this
/// function re-accepting it — threading it through here just to gate on it
/// internally would be the "the whole function behind `#[cfg]`" shape this
/// contract exists to avoid.
fn stage_frame_damage(
    win: &PerWindowState,
    ctx: &egui::Context,
    pane_layout: &[(panes::PaneId, egui::Rect)],
    active_pane_id: panes::PaneId,
    inputs: &FrameDamageInputs,
    toasts: &std::cell::RefCell<super::toast::ToastStack>,
) -> (freminal_windowing::FrameDamage, FrameDamageObservations) {
    // A window-post shader recomposites the entire window every frame, so it
    // forces `Full`.
    let shader_recomposites = {
        let wpr = win
            .window_post
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        wpr.is_active() || wpr.pending_shader.is_some()
    };
    // The pane-border active-pane highlight, menu/tab-bar hover tints, and
    // other chrome are painted by the plain egui painter every frame —
    // outside the per-pane damage tracking. They only *change* pixels when
    // the active pane changes (border moves) or the pointer moves (hover).
    // Presenting only the cursor rect on such a frame would leave that
    // chrome stale, so both force `Full`.
    let pointer_moving = ctx.input(|i| i.pointer.is_moving());
    // #459 item 9: pointer motion only changes plain-egui-painter pixels
    // when it is over a chrome-interactive region (menu/tab-bar hover
    // tints, pane-border highlight) or an active pane-border drag is in
    // progress (the drag may have moved the pointer off the ±3px sensor
    // mid-drag). Motion purely over terminal content changes no
    // egui-painted chrome — the terminal band tracks its own hover effects
    // (gutter tint, URL cursor, scrollbar thumb) through explicit per-pane
    // damage signals. NB: `self.is_chrome_interactive_at` is UNUSABLE here
    // — `win` was removed from `self.windows` for this frame (see the
    // `self.windows.remove` at the top of `update`), so it would always
    // return `true`; hit-test the local `win` rects directly.
    let pointer_over_chrome = ctx.input(|i| i.pointer.latest_pos()).is_none_or(|pos| {
        chrome_damage::point_in_chrome_rects(
            pos,
            win.published.chrome_head_rects(),
            win.published.chrome_border_rects(),
            win.published.chrome_toast_rects(),
        )
    });
    let force_full = inputs.ui_overlay_open
        || shader_recomposites
        || inputs.active_pane_changed
        || pointer_forces_full_present(
            pointer_moving,
            pointer_over_chrome,
            inputs.border_drag_active,
        );
    // A toast being visible animates its own region each frame. The resize
    // overlay (issue #433) animates the same way — it fades out over its
    // linger window on the plain painter — so it must force a `Full`
    // present too, or a cursor-only `Partial` frame would leave the fading
    // overlay outside the damage rect stale/ghosted.
    let toast_active = win.published.resize_overlay().is_some()
        || toasts.try_borrow().is_ok_and(|stack| !stack.is_empty());
    // Inspect only the panes actually rendered this frame — the entries in
    // `pane_layout`. Under zoom, only the zoomed pane is rendered;
    // iterating the whole tree would read stale `last_frame_cursor_damage`
    // from non-rendered siblings and wrongly force `Full` every frame. The
    // per-pane -> `FrameDamage` decision itself (and its full
    // case-by-case rationale) is extracted into `decide_frame_damage`
    // (#436.2b) so both this path and the future REPLAY path compute it
    // identically.
    let active_tab = win.tabs.active_tab();
    let mut unresolved_pane = false;
    // OR-accumulated across every rendered pane: does ANY of them have an
    // open overlay that paints ABOVE the terminal band — the
    // `Order::Foreground` context menu, in-terminal search bar, or
    // command-history palette, OR the `Order::Tooltip` URL-hover tooltip?
    // All of these paint as TAIL chrome outside the captured terminal-band
    // range, so a REPLAY frame (which reuses the stale cached tail) must
    // not be permitted while one is open, or it would vanish/ghost
    // (#436.4b fix — see `ChromeSignals::foreground_overlay_open`). The URL
    // tooltip is driven by `render_cache.cached_hovered_url`, which is
    // recomputed even under a STATIONARY mouse when PTY output scrolls new
    // content under the cursor — i.e. it can change on a frame with no
    // window input event, exactly the frame a REPLAY would otherwise be
    // chosen.
    let mut foreground_overlay_open = false;
    // Diagnostic: capture the active pane's per-frame damage class (reused
    // from the #435 signal) for the frame-attribution stats flushed by the
    // caller; `None` if the active pane isn't resolved.
    let mut active_pane_damage: Option<crate::gui::renderer::PaneFrameDamage> = None;
    let mut per_pane_damage: Vec<frame_damage::PaneDamageInput> =
        Vec::with_capacity(pane_layout.len());
    for (pane_id, _) in pane_layout {
        let Some(pane) = active_tab.pane_tree.find(*pane_id) else {
            // A pane in the layout we cannot resolve -> be safe. This also
            // aborts the `foreground_overlay_open` scan before every pane
            // has been inspected, so conservatively treat an unresolved
            // pane as if a foreground overlay were open — it already
            // forces `FrameDamage::Full` below, and the chrome decision
            // must be at least as conservative.
            unresolved_pane = true;
            foreground_overlay_open = true;
            break;
        };
        foreground_overlay_open |= pane.view_state.context_menu_pos.is_some()
            || pane.view_state.search_state.is_open
            || pane.view_state.command_history.is_open
            || pane.render_cache.hover_tooltip_active();
        if *pane_id == active_pane_id {
            active_pane_damage = Some(pane.render_cache.last_frame_cursor_damage);
        }
        per_pane_damage.push(frame_damage::PaneDamageInput {
            bell_active: pane.view_state.bell_since.is_some(),
            cursor_damage: pane.render_cache.last_frame_cursor_damage,
        });
    }
    let decided = frame_damage::decide_frame_damage(
        force_full || unresolved_pane,
        toast_active,
        &per_pane_damage,
    );

    (
        decided,
        FrameDamageObservations {
            shader_recomposites,
            #[cfg(feature = "frame-profiling")]
            force_full,
            #[cfg(feature = "frame-profiling")]
            unresolved_pane,
            toast_active,
            foreground_overlay_open,
            per_pane_damage,
            active_pane_damage,
        },
    )
}

/// Per-frame §3.3 boolean signals [`stage_chrome_signals`] needs from its
/// caller, computed earlier in `central_body` from state this function has
/// no access to (the style-cache comparison, the previous-active-pane
/// cache, the #435 `stage_frame_damage` observations, DPI/focus/size
/// bookkeeping, and the #436 warm-up counter). Grouped into a params
/// struct to stay under clippy's `too_many_arguments` threshold —
/// [`FrameDamageInputs`] (above, in this same file) is the in-tree
/// precedent for this pattern. Each field is an independent,
/// simultaneously-observable per-frame signal — the same "independent
/// simultaneous signals" exemption `chrome_damage::ChromeSignals` itself
/// documents, not a state machine — so grouping in a struct rather than
/// converting to enums is correct (`freminal-state-representation`).
///
/// Field names deliberately mirror the local variables at the
/// `stage_chrome_signals` call site (not `ChromeSignals`' own field names)
/// so the forwarding assignment inside `stage_chrome_signals` is a visible
/// rename, not a silent one.
#[allow(clippy::struct_excessive_bools)]
struct ChromeSignalInputs {
    /// Forwarded to `ChromeSignals::any_overlay_open`.
    ui_overlay_open: bool,
    /// Forwarded to `ChromeSignals::style_changed`.
    chrome_style_changed: bool,
    /// Forwarded to `ChromeSignals::active_pane_changed`.
    active_pane_changed: bool,
    /// Forwarded to `ChromeSignals::shader_active`.
    shader_recomposites: bool,
    /// Forwarded to `ChromeSignals::toast_active`.
    toast_active: bool,
    /// Forwarded to `ChromeSignals::size_changed`.
    chrome_size_changed: bool,
    /// Forwarded to `ChromeSignals::ppp_changed`.
    ppp_changed: bool,
    /// Forwarded to `ChromeSignals::focus_changed`.
    chrome_focus_changed: bool,
    /// Forwarded to `ChromeSignals::warming_up`.
    chrome_warming_up: bool,
    /// Forwarded to `ChromeSignals::foreground_overlay_open`.
    foreground_overlay_open: bool,
}

/// Stage this frame's #436.3 [`chrome_damage::ChromeTabSnapshot`] and the
/// [`chrome_damage::ChromeSignals`] it partly feeds, extracted from
/// `App::update`'s `central_body` closure (Task 122.10).
///
/// Returns both values rather than assigning them: the caller is
/// responsible for replacing `win.prev_chrome_tab_snapshot` with the
/// returned snapshot (next frame's diff baseline) and for handing the
/// returned `ChromeSignals` to
/// `PublishedChromeState::publish_pending_chrome_signals` — the final
/// `ChromeDamage` decision additionally needs the after-toast-render
/// dismissible-presence sample, which can only be taken once
/// `central_body` returns (the toast overlay renders after it), so this
/// function only stages the signals; it does not decide or publish
/// anything. This mirrors [`stage_frame_damage`]'s "return, don't mutate"
/// shape (122.9) for the same reason: keeping both writes visible at the
/// call site.
///
/// `bell_active` and `active_tab_id` are computed here, inline, exactly as
/// in the original block, rather than threaded in as extra parameters:
/// `bell_active` folds `per_pane_damage` (already a parameter, so no
/// re-evaluation risk) and `active_tab_id` reads `win.tabs.active_tab().id`
/// directly. Hoisting either out to the caller would only add an
/// opportunity for it to be evaluated at a different point relative to the
/// snapshot build than it is today.
fn stage_chrome_signals(
    win: &PerWindowState,
    tab_title_policy: freminal_common::config::TabTitlePolicy,
    tab_title_separator: &str,
    pane_layout: &[(panes::PaneId, egui::Rect)],
    zoomed_pane: Option<panes::PaneId>,
    per_pane_damage: &[frame_damage::PaneDamageInput],
    inputs: &ChromeSignalInputs,
) -> (
    chrome_damage::ChromeTabSnapshot,
    chrome_damage::ChromeSignals,
) {
    let chrome_tab_snapshot = chrome_damage::ChromeTabSnapshot {
        tab_ids: win.tabs.iter().map(|t| t.id).collect(),
        active_tab_id: Some(win.tabs.active_tab().id),
        tab_titles: win
            .tabs
            .iter()
            .map(|t| {
                t.display_name(tab_title_policy, tab_title_separator)
                    .into_owned()
            })
            .collect(),
        pane_ids: pane_layout.iter().map(|(id, _)| *id).collect(),
        zoomed_pane,
        broadcast_input: win.tabs.active_tab().broadcast_input,
    };
    let chrome_tab_diff =
        chrome_damage::diff_tab_snapshots(&win.prev_chrome_tab_snapshot, &chrome_tab_snapshot);

    let chrome_signals = chrome_damage::ChromeSignals {
        any_overlay_open: inputs.ui_overlay_open,
        style_changed: inputs.chrome_style_changed,
        active_pane_changed: inputs.active_pane_changed,
        tab_set_changed: chrome_tab_diff.tab_set_changed,
        tab_title_changed: chrome_tab_diff.tab_title_changed,
        pane_layout_changed: chrome_tab_diff.pane_layout_changed,
        broadcast_state_changed: chrome_tab_diff.broadcast_state_changed,
        shader_active: inputs.shader_recomposites,
        bell_active: per_pane_damage.iter().any(|p| p.bell_active),
        toast_active: inputs.toast_active,
        size_changed: inputs.chrome_size_changed,
        ppp_changed: inputs.ppp_changed,
        focus_changed: inputs.chrome_focus_changed,
        warming_up: inputs.chrome_warming_up,
        foreground_overlay_open: inputs.foreground_overlay_open,
    };

    (chrome_tab_snapshot, chrome_signals)
}

impl freminal_windowing::App for FreminalGui {
    /// Called when a window is created.
    ///
    /// For the first window, consumes `initial_state` to get the pre-spawned
    /// tab and widget.  For subsequent windows, spawns a fresh PTY tab.
    // Window creation handles two distinct paths (first window with pre-spawned state vs
    // subsequent windows with fresh PTY) that share no logic — splitting would not reduce
    // coupling and would obscure the flow.
    #[allow(clippy::too_many_lines)]
    fn on_window_created(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
    ) {
        // ── Settings window ──────────────────────────────────────────────────
        if self.pending_settings_window {
            self.pending_settings_window = false;
            self.settings_window_id = Some(window_id);
            // `settings_owner` already holds the *terminal* window that
            // requested this settings window (set by the menu/keybind
            // action before `handle.create_window()` was called). Do NOT
            // overwrite it with this settings window's own id here — doing
            // so used to make `settings_owner == settings_window_id`
            // always, which broke the owning-window-close guard (issue
            // #401: the guard compares `settings_owner` against a real
            // terminal window id, which could then never match) and the
            // "Test Paste" routing / os_dark_mode lookup, both of which key
            // off `settings_owner` to find the owning terminal window.
            // Don't create a PerWindowState — the settings window renders
            // only the settings UI via show_standalone().
            return;
        }

        let os_dark_mode = ctx.global_style().visuals.dark_mode;

        if let Some(initial) = self.initial_state.take() {
            // Start the periodic session auto-save timer, bound to the first
            // window's repaint handle so it can wake the (otherwise sleeping)
            // event loop when a save is due.  Spawned exactly once, here at
            // first-window creation.
            self.spawn_session_autosave_timer(Arc::clone(&initial.repaint_handle));

            // First window — spawn the initial PTY tab now, or if a
            // startup layout/session-restore applies, delegate to the
            // layout machinery (which will build the tabs itself and
            // avoid a throwaway PTY spawn).
            if self.will_layout_or_restore_apply() {
                self.create_first_window_from_layout_or_restore(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                    initial.repaint_handle,
                    initial.window_post,
                );
            } else {
                self.create_first_window_with_default_pty(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                    initial.repaint_handle,
                    initial.window_post,
                );
            }

            self.emit_window_create_recording(window_id, inner_size);
        } else {
            // Subsequent window — check if a layout window is waiting, otherwise
            // spawn a default single-pane PTY tab.
            if !self.pending_layout_windows.is_empty() {
                if let Some(cmds) = self.build_window_from_pending_layout(
                    window_id,
                    ctx,
                    handle,
                    inner_size,
                    os_dark_mode,
                    None,
                ) {
                    self.inject_layout_commands(&cmds);
                }
                return;
            }

            // Subsequent window — spawn a new PTY tab.
            let theme =
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            rendering::set_egui_options(
                ctx,
                theme,
                self.config.ui.background_opacity,
                &self.gui_theme,
            );

            let repaint_handle = Arc::new(OnceLock::new());
            let proxy = handle.event_loop_proxy();
            let _ = repaint_handle.set((proxy, window_id));

            let window_post = Arc::new(Mutex::new(WindowPostRenderer::new()));

            let terminal_widget =
                FreminalTerminalWidget::new(ctx, &self.config).unwrap_or_else(|e| {
                    tracing::error!(
                        "fatal: failed to initialise terminal widget (font manager): {e}"
                    );
                    std::process::exit(1);
                });
            let (cell_w, cell_h) = terminal_widget.cell_size();
            let initial_size =
                Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

            let pane_id = self
                .pane_id_gen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .next_id();

            match super::pty::spawn_pty_tab(
                &self.args,
                self.config.scrollback.limit,
                super::pty::PtyTabInitialState {
                    theme,
                    auto_detect_urls: self.config.ui.auto_detect_urls,
                    cursor_style: freminal_common::cursor::CursorVisualStyle::from_config(
                        &self.config.cursor.shape,
                        self.config.cursor.blink,
                    ),
                },
                &repaint_handle,
                initial_size,
                super::pty::PtyTabConfig {
                    cwd: None,
                    shell_override: None,
                    extra_env: None,
                    recording_swap: self.recording_swap.clone(),
                    recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
                    set_term_program: self.config.shell_integration.set_term_program,
                },
            ) {
                Ok(channels) => {
                    let pane = panes::Pane::from_channels(
                        pane_id,
                        channels,
                        Arc::clone(&window_post),
                        "Terminal".to_owned(),
                    );
                    let tab_id = super::tabs::TabId::first();
                    let tab = Tab::new(tab_id, pane);

                    if let Some(active) = tab.active_pane() {
                        if let Err(e) = active.input_tx.send(InputEvent::ThemeModeUpdate(
                            self.config.theme.mode,
                            os_dark_mode,
                        )) {
                            error!("Failed to send ThemeModeUpdate to new window tab: {e}");
                        }
                    } else {
                        warn!("new window tab has no active pane when sending ThemeModeUpdate");
                    }

                    // Copy shader from config if present.
                    let shader_src = self
                        .config
                        .shader
                        .path
                        .as_ref()
                        .and_then(|p| std::fs::read_to_string(p).ok());
                    if let Some(src) = shader_src
                        && let Ok(mut wpr) = window_post.lock()
                    {
                        wpr.pending_shader = Some(Some(src));
                    }

                    // Copy bg image if present.
                    let bg_path = self.config.ui.background_image.clone();
                    if bg_path.is_some()
                        && let Ok(panes_list) = tab.pane_tree.iter_panes()
                    {
                        for p in panes_list {
                            if let Ok(mut rs) = p.render_state.lock() {
                                rs.set_pending_bg_image(bg_path.clone());
                            }
                        }
                    }

                    let win = PerWindowState {
                        tabs: TabManager::new(tab),
                        terminal_widget,
                        last_window_title: String::from("Freminal"),
                        os_dark_mode,
                        style_cache: None,
                        pending_close_pane: false,
                        pending_focus_direction: None,
                        border_drag: None,
                        published: super::published_frame_state::PublishedFrameState::new(),
                        shader_last_mtime: None,
                        window_post,
                        toast_render_state: crate::gui::renderer::ToastRenderState::new_shared(),
                        repaint_handle,
                        pending_new_window: false,
                        pending_geometry: None,
                        last_known_size: None,
                        last_known_position: None,
                        renaming_tab: None,
                        rename_buffer: String::new(),
                        dragging_tab: None,
                        last_tab_rects: Vec::new(),
                        pending_menu_actions: Vec::new(),
                        paste_dialog: super::paste_guard::PasteDialog::default(),
                        broadcast_dialog: super::broadcast_guard::BroadcastConfirmDialog::default(),
                        close_dialog: super::close_guard::CloseGuardDialog::default(),
                        pending_force_close: false,
                        pending_raw_keys: Vec::new(),
                        pending_frame_damage: freminal_windowing::FrameDamage::Full,
                        pending_terminal_band_range: 0..0,
                        present_is_partial: std::sync::Arc::new(
                            std::sync::atomic::AtomicBool::new(false),
                        ),
                        previous_active_pane_key: None,
                        pending_chrome_damage: freminal_windowing::ChromeDamage::Changed,
                        chrome_settle_pending: false,
                        prev_dismissible_presence: chrome_damage::DismissiblePresence::default(),
                        prev_chrome_tab_snapshot: chrome_damage::ChromeTabSnapshot::default(),
                        prev_window_focused: false,
                        chrome_frames_rendered: 0,
                        pending_terminal_requested_delay: None,
                        frame_stats: super::window::FrameStats::default(),
                    };
                    self.windows.insert(window_id, win);

                    self.emit_window_create_recording(window_id, inner_size);
                }
                Err(e) => {
                    error!("Failed to spawn PTY for new window: {e}");
                    self.push_error_toast(
                        "Failed to open new window",
                        Some(format!("The shell could not be started: {e}")),
                    );
                }
            }
        }
    }

    /// Called when a window close is requested.
    ///
    /// Removes the window's state — its PTY threads will be dropped when
    /// the channels close. Returns `false` to veto the close: either the
    /// settings modal has unsaved edits to confirm (this window's own
    /// close, or the owning terminal window's — issue #401), or the window
    /// has a running foreground command pending confirmation (Task 98).
    fn on_close_requested(&mut self, window_id: WindowId) -> bool {
        // Settings window closed (via OS close button).
        if self.settings_window_id == Some(window_id) {
            // Consult the unsaved-changes guard.  When dirty, the modal
            // surfaces a confirm prompt on its next frame; veto the OS close
            // so the window stays open long enough for the user to decide.
            if !self.settings_modal.request_close() {
                return false;
            }
            self.settings_modal.is_open = false;
            self.settings_window_id = None;
            self.settings_owner = None;
            self.persist_window_state();
            return true;
        }
        // If this window owns an open Settings window, decide what to do
        // about it before this window closes (issue #401: the settings
        // window used to be orphaned — left open with its internal `is_open`
        // flag force-cleared but the actual OS window never told to close,
        // freezing it, and never counted against the "all windows closed"
        // quit check).
        match settings_owner_close_decision(
            self.settings_owner == Some(window_id),
            self.settings_modal.has_unsaved_changes(),
        ) {
            SettingsOwnerCloseDecision::NotOwner => {}
            SettingsOwnerCloseDecision::CloseNow => {
                self.settings_modal.is_open = false;
                self.settings_owner = None;
                // Nothing else will wake the settings window once its owner
                // is gone — force a repaint so its own next `update()` call
                // notices `is_open == false` and closes the OS window
                // itself via the existing self-close path (see `update()`'s
                // settings-window branch).
                if let Some(sid) = self.settings_window_id
                    && let Some((proxy, _)) = self
                        .windows
                        .get(&window_id)
                        .and_then(|w| w.repaint_handle.get())
                {
                    proxy.request_repaint(sid);
                }
            }
            SettingsOwnerCloseDecision::VetoWithPrompt => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    win.close_dialog.open(super::close_guard::PendingClose {
                        scope: super::close_guard::CloseScope::WindowUnsavedSettings,
                        running: Vec::new(),
                    });
                }
                return false;
            }
        }

        // Close-on-running-command guard (Task 98.7).  If the user already
        // confirmed a force-close for this window, let it through and clear
        // the flag.  Otherwise, if any pane in the window has a running
        // foreground command, open the confirmation dialog and veto the OS
        // close (return false); the dialog's Force Close re-issues the close
        // with this flag set.
        if self.force_close_windows.remove(&window_id) {
            // User-confirmed force close — fall through to the close logic.
        } else if let Some(win) = self.windows.get(&window_id) {
            let running = self.window_close_running(win);
            if !running.is_empty()
                && let Some(win) = self.windows.get_mut(&window_id)
            {
                win.close_dialog.open(super::close_guard::PendingClose {
                    scope: super::close_guard::CloseScope::Window,
                    running,
                });
                return false;
            }
        }

        // Auto-save session before the last terminal window is removed.
        // We check *before* remove so we still have access to the window's tabs.
        //
        // Saving is independent of `restore_last_session` — the flag only
        // controls whether the saved session is *applied* on next launch.
        // Saving keeps `last_session.toml` fresh so users can toggle the flag
        // on at any time and get their real last session back, rather than
        // whatever stale state happened to be on disk when they last had the
        // flag enabled.
        //
        // `maybe_auto_save_session` skips the write when nothing changed since
        // the periodic timer last persisted, and skips entirely for ad-hoc
        // command launches (`freminal -- vim foo`).  In the common case the
        // periodic save already wrote the current state, so this shutdown call
        // is a no-op — by design, so we no longer depend on a write surviving
        // an abrupt teardown.
        let remaining_terminal_windows = self
            .windows
            .keys()
            .filter(|&&wid| Some(wid) != self.settings_window_id)
            .count();
        if remaining_terminal_windows == 1 {
            self.maybe_auto_save_session();
        }

        // Capture geometry of every still-open terminal window (including
        // the one being closed) into `window_state.main_windows`, with the
        // closing window first so it seeds the primary window on next
        // launch.  Persist unconditionally — this is independent of
        // `restore_last_session`.
        self.snapshot_main_window_geometry(Some(window_id));
        self.persist_window_state();

        self.windows.remove(&window_id);

        // Emit WindowClose recording event (only for known windows), and clean up the mapping.
        if let Some(rec_wid) = self.recording_window_ids.remove(&window_id)
            && let Some(h) = self.recording_swap.load_full()
        {
            h.emit(
                freminal_terminal_emulator::recording::EventPayload::WindowClose {
                    window_id: rec_wid,
                },
            );
        }

        true
    }

    /// Override the GL framebuffer clear color.
    ///
    /// When `background_opacity < 1.0` the viewport was created with
    /// `transparent = true`, so the compositor can show the desktop through.
    /// For that to work the clear color must have alpha = 0; otherwise the
    /// opaque clear overwrites the transparent framebuffer before egui
    /// paints anything.
    ///
    /// When opacity is 1.0 the clear color matches `panel_fill` (fully
    /// opaque) — there is no visible difference from the default.
    fn clear_color(&self, window_id: WindowId) -> [f32; 4] {
        // Settings window: use a neutral opaque background.
        if self.settings_window_id == Some(window_id) {
            return [0.2, 0.2, 0.2, 1.0];
        }
        if self.config.ui.background_opacity < 1.0 {
            [0.0, 0.0, 0.0, 0.0]
        } else {
            // Fully opaque: use the terminal background color from the theme.
            // Honor the live preview override so the window background tracks a
            // theme being previewed in Settings, not just the committed config.
            let os_dark_mode = self.windows.get(&window_id).is_some_and(|w| w.os_dark_mode);
            let theme = self.preview_theme.unwrap_or_else(|| {
                freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA)
            });
            let (r, g, b) = theme.background;
            let color = egui::Color32::from_rgb(r, g, b);
            color.to_normalized_gamma_f32()
        }
    }

    fn present_partial_flag(
        &self,
        window_id: WindowId,
    ) -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
        self.windows
            .get(&window_id)
            .map(|win| std::sync::Arc::clone(&win.present_is_partial))
    }

    fn is_chrome_interactive_at(&self, window_id: WindowId, pos: egui::Pos2) -> bool {
        self.windows.get(&window_id).is_none_or(|win| {
            crate::gui::chrome_damage::point_in_chrome_rects(
                pos,
                win.published.chrome_head_rects(),
                win.published.chrome_border_rects(),
                // Subtask 121.14 (review item 2 follow-up): the most
                // recently laid-out toast pill rects — see
                // `PublishedFrameState`'s doc for the staleness discipline.
                // Hovering a toast DOES change chrome pixels (close-button
                // highlight, hover-pauses-expiry), so this correctly, as a
                // consequence, also makes `should_force_chrome_full_for_pointer`
                // (in `freminal-windowing`, which calls this method) force
                // `ChromeMode::Full` while the pointer is over a toast.
                win.published.chrome_toast_rects(),
            )
        })
    }

    /// Task 121 pointer-motion repaint-gate spike: wires real per-window/
    /// per-pane state into [`pointer_motion_needs_repaint_decision`]. See
    /// that function's doc for the exact forcing conditions, and
    /// `pointer_motion::pane_hover_region_risk`'s doc for the residual-risk
    /// approximation used for URL/gutter/scrollbar hover regions.
    ///
    /// Reuses `win.published`'s `pending_chrome_signals().any_overlay_open`/
    /// `foreground_overlay_open` — the persisted #436 §3.3 signals from the
    /// most recently rendered frame — for the "any overlay/popup/tooltip/
    /// context menu is open" bullet, rather than recomputing the
    /// `ui_overlay_open`/`foreground_overlay_open` scans (`update()`'s
    /// `CentralPanel` closure, around `app_impl.rs` lines 1841 and
    /// 2708-2733): this method runs OUTSIDE any frame (from `event_loop`'s
    /// `CursorMoved` handling), where that scan's inputs (menu state,
    /// dialog `Ui`s, etc.) are not in scope.
    ///
    /// ## Gutter positional test (Task 121 fix)
    ///
    /// The pane-level `gutter_active` term used to be pane-wide
    /// (`gutter_config_active && !alt_screen && !command_blocks.is_empty()`),
    /// which measured at 100% fired on every pointer-motion check in any
    /// session with auto-detected command blocks (Task 121 spike), making
    /// suppression a no-op. The gutter is actually a narrow strip on the
    /// pane's LEFT edge (`terminal/widget.rs`'s `gutter_rect`, built from
    /// `pane_rect.min.x .. pane_rect.min.x + gutter.width_px() / ppp`), so
    /// this method now also tests `pos.x` against that strip using the
    /// SAME pane rect this method already resolves for pane hit-testing
    /// (`layout`, below) — no separate rect computation.
    ///
    /// `pixels_per_point` (`ppp`) is not available at this call site (it is
    /// only known once egui begins a frame; this method runs from
    /// `event_loop`'s `CursorMoved` handling, outside any frame), and
    /// `PerWindowState` does not cache it — adding that cache purely for
    /// this one comparison would be new per-frame machinery for a single
    /// read. Instead this uses `gutter.width_px()` (physical pixels)
    /// DIRECTLY as an upper bound on the logical strip width
    /// `width_px / ppp`: since `ppp >= 1.0` on every realistic display,
    /// `width_px / ppp <= width_px`, so testing `pos.x < pane_rect.min.x +
    /// width_px` only ever widens the strip relative to the real
    /// (smaller-or-equal) one — it can cause the gutter term to fire when
    /// the real strip would not have (false positive => an unnecessary
    /// repaint, never a missed one), which is the safe direction for a
    /// repaint gate.
    ///
    /// Subtask 122.5: the pane-resolution chain itself (layout -> hit-test
    /// -> snapshot lookup -> signal computation) now lives in the pure,
    /// headlessly-testable [`resolve_pane_under_pointer`] — see that
    /// function's doc for the zoomed-vs-split mirroring requirement this
    /// paragraph used to describe inline.
    fn pointer_motion_needs_repaint(&self, window_id: WindowId, pos: egui::Pos2) -> bool {
        let Some(win) = self.windows.get(&window_id) else {
            // Unknown window -> conservative (mirrors `is_chrome_interactive_at`).
            return true;
        };

        let chrome_interactive = self.is_chrome_interactive_at(window_id, pos);
        // Animation-in-flight terms. The rest of this predicate is *positional*
        // ("does the pointer's current position matter?"), which is structurally
        // blind to "something is mid-animation somewhere in this window,
        // regardless of where the pointer is". Toasts and the resize-overlay HUD
        // are the two things that can animate independent of pointer position;
        // without these terms, wandering the pointer over plain terminal content
        // during one of their fades would let the `RedrawRequested` override
        // substitute the app's much longer delay for the animation's cadence,
        // making the fade visibly step instead of animate. Bounded (<=500ms)
        // rather than a freeze, but a real visual regression reachable from an
        // already-shipped feature.
        //
        // Subtask 121.14: both terms below test ANIMATION, not PRESENCE — the
        // superset-but-wasteful bug this subtask fixes. A toast spends most of
        // its life fully settled in its steady hold; the resize HUD is fully
        // opaque for the first `RESIZE_OVERLAY_LINGER - RESIZE_OVERLAY_FADE` of
        // its life. Presence alone (the old `win.published.resize_overlay().is_some()` /
        // `!stack.is_empty()`) disabled suppression for their entire lives
        // instead of just the genuinely-animating tail.
        let resize_overlay_animating = win.published.resize_overlay().is_some_and(|overlay| {
            let elapsed = std::time::Instant::now().saturating_duration_since(overlay.last_update);
            super::window::resize_overlay_is_animating(
                elapsed,
                super::window::RESIZE_OVERLAY_LINGER,
                super::window::RESIZE_OVERLAY_FADE,
            )
        });
        // `try_borrow` failing means someone up-stack holds the `RefCell`; treat
        // that as "toasts may be animating" (conservative), never as "no
        // toasts"/"not animating".
        let toast_animating = self
            .toasts
            .try_borrow()
            .map_or(true, |stack| !stack.is_empty() && stack.is_animating());
        let animation_in_flight =
            animation_in_flight_composed(resize_overlay_animating, toast_animating);
        let overlay_open = win.published.pending_chrome_signals().any_overlay_open
            || win
                .published
                .pending_chrome_signals()
                .foreground_overlay_open
            || animation_in_flight;

        let active_tab = win.tabs.active_tab();
        // `PaneError::InvalidState` (empty tree) is a bug state that should
        // never happen in normal operation -- conservative true on `Err`.
        let any_selecting = active_tab.pane_tree.iter_panes().map_or(true, |panes| {
            panes
                .iter()
                .any(|pane| pane.view_state.selection.is_selecting)
        });

        let pointer_pane_unresolved = win.published.cached_central_rect().is_none();

        let gutter_config_active = self.config.command_blocks.enabled
            && self.config.command_blocks.gutter != freminal_common::config::GutterPosition::Off;

        // The gutter's total inset in LOGICAL points, cached by `update()`
        // (this method runs outside a frame and has no `ppp`). The inset is
        // strictly wider than the painted strip, so it is a conservative
        // bound by construction — and unlike the previous
        // physical-pixels-as-logical approximation it does not depend on
        // `ppp >= 1.0`, whose safety direction inverts on sub-1.0 fractional
        // scale. See `PublishedFrameState::cached_gutter_inset_logical`.
        let gutter_width_upper_bound_logical = win.published.cached_gutter_inset_logical();

        // Subtask 122.5: the pane-resolution chain (layout -> hit-test ->
        // snapshot lookup -> signal computation) is now the pure,
        // headlessly-testable `resolve_pane_under_pointer`. It also
        // computes the four diagnostic term bools unconditionally -- see
        // `PaneResolution`'s doc for why that, rather than the previous
        // `#[cfg(feature = "frame-profiling")]` block interleaved with the
        // computation, is what makes the recording below structurally
        // unable to drift from the real decision.
        let pane_resolution = win.published.cached_central_rect().map_or_else(
            PaneResolution::unresolved,
            |central_rect| {
                // Mirror `update()`'s own zoomed-vs-split layout choice
                // exactly: when a pane is zoomed, the split layout below is
                // never built (matching `update()`, which also skips it in
                // that case) and `resolve_pane_under_pointer` treats the
                // zoomed pane as filling `central_rect` instead. The actual
                // zoomed-vs-split CHOICE lives inside that pure function;
                // this closure only supplies the data for the non-zoomed
                // case.
                let split_layout: Vec<(panes::PaneId, Rect)> =
                    if active_tab.zoomed_pane.is_none() {
                        active_tab
                            .pane_tree
                            .layout(geometry_interop::rect_from_egui(central_rect))
                            .unwrap_or_default()
                    } else {
                        Vec::new()
                    };

                resolve_pane_under_pointer(
                    geometry_interop::point_from_egui(pos),
                    geometry_interop::rect_from_egui(central_rect),
                    active_tab.zoomed_pane,
                    &split_layout,
                    gutter_config_active,
                    gutter_width_upper_bound_logical,
                    |pane_id| {
                        active_tab.pane_tree.find(pane_id).map(|pane| {
                            let snap = pane.arc_swap.load();
                            PaneSnapshotInputs {
                                mouse_tracking_active: snap.mouse_tracking
                                    != freminal_common::buffer_states::modes::mouse::MouseTrack::NoTracking,
                                has_urls: snap.has_urls,
                                scroll_offset: snap.scroll_offset,
                                is_alternate_screen: snap.is_alternate_screen,
                                command_blocks_non_empty: !snap.command_blocks.is_empty(),
                            }
                        })
                    },
                )
            },
        );

        // Task 121 diagnostic: count which condition(s) fired for this
        // call, `saturating_add`'d into `win.frame_stats`'s Task 121
        // counters (see `FrameStats::record_pointer_motion_check`'s doc for
        // why `Cell` makes this possible through `win`'s immutable
        // borrow). Read out, logged, and reset every `FLUSH_EVERY` drawn
        // frames from the app-side flush further down in `update()`.
        // Counting only -- does not read from or influence
        // `pointer_motion_needs_repaint_decision`'s return value below.
        // Subtask 122.5: only the RECORDING stays feature-gated -- the four
        // terms it reads were computed unconditionally by
        // `resolve_pane_under_pointer`, above.
        #[cfg(feature = "frame-profiling")]
        {
            win.frame_stats.record_pointer_motion_check(
                super::window::PointerMotionConditionFlags {
                    chrome_interactive,
                    any_pane_selecting: any_selecting,
                    overlay_open,
                    pointer_pane_unresolved,
                    mouse_tracking_active: pane_resolution.mouse_tracking_active,
                    has_urls: pane_resolution.has_urls,
                    scroll_offset_nonzero: pane_resolution.scroll_offset_nonzero,
                    gutter_active: pane_resolution.gutter_active,
                },
            );
        }

        // Focus-follows-mouse turns pointer motion into a state change, so
        // the gate must not suppress the frame that would apply it (#495).
        // Narrow by construction: only motion that lands on a pane other than
        // the active one qualifies, so moving around inside the focused pane
        // still suppresses exactly as before.
        let focus_change_pending = self.config.tabs.focus_follows_mouse
            && pane_resolution
                .resolved_pane
                .is_some_and(|id| id != active_tab.active_pane);

        pointer_motion_needs_repaint_decision(
            focus_change_pending,
            chrome_interactive,
            any_selecting,
            overlay_open,
            pointer_pane_unresolved,
            pane_resolution.signals,
        )
    }

    fn take_frame_damage(&mut self, window_id: WindowId) -> freminal_windowing::FrameDamage {
        // Drain the damage computed during `update()` for this window, leaving
        // `Full` behind so a stale value can never be reused on a later frame
        // that does not recompute it.
        self.windows
            .get_mut(&window_id)
            .map_or(freminal_windowing::FrameDamage::Full, |win| {
                std::mem::replace(
                    &mut win.pending_frame_damage,
                    freminal_windowing::FrameDamage::Full,
                )
            })
    }

    fn take_terminal_band_range(&mut self, window_id: WindowId) -> std::ops::Range<usize> {
        // Drain the terminal-band range captured during `update()` for this
        // window, leaving `0..0` behind so a stale frame's range can never
        // be reused by a later caller that does not recompute it.
        self.windows.get_mut(&window_id).map_or(0..0, |win| {
            std::mem::replace(&mut win.pending_terminal_band_range, 0..0)
        })
    }

    fn take_chrome_damage(&mut self, window_id: WindowId) -> freminal_windowing::ChromeDamage {
        // Drain the chrome-damage decision computed during `update()` for
        // this window, leaving `Changed` behind so a stale `Unchanged` can
        // never be reused by a later frame that does not recompute it.
        self.windows
            .get_mut(&window_id)
            .map_or(freminal_windowing::ChromeDamage::Changed, |win| {
                std::mem::replace(
                    &mut win.pending_chrome_damage,
                    freminal_windowing::ChromeDamage::Changed,
                )
            })
    }

    fn take_terminal_requested_delay(
        &mut self,
        window_id: WindowId,
    ) -> Option<std::time::Duration> {
        // Drain the delay `update()` itself requested via
        // `ctx.request_repaint_after` this window's most recent frame,
        // leaving `None` behind so a stale delay can never be reused by a
        // later frame that does not recompute it.
        self.windows
            .get_mut(&window_id)
            .and_then(|win| win.pending_terminal_requested_delay.take())
    }

    // Inherently large: the main per-frame UI function handles menu bar, settings modal, window
    // manipulation drain, terminal widget layout, and resize detection — all in one pass over
    // the shared snapshot. Artificial sub-functions would not reduce the coupling.
    #[allow(clippy::too_many_lines)]
    fn update(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        _gl: &glow::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        chrome_mode: freminal_windowing::ChromeMode,
    ) {
        trace!("Starting new frame");
        let now = std::time::Instant::now();
        // Task 121 frame-profiling harness (defect-1 fix): captured at the
        // VERY top of `update`, before either early-return branch below, so
        // its eventual `.elapsed()` (taken after `compose_with_chrome_damage`,
        // near the end of this function) covers the whole productive body of
        // `App::update` -- not just the `central_body` closure. The three
        // early-return paths below (settings-window dispatch, dead-window
        // cleanup, no-active-pane) intentionally never reach that `.elapsed()`
        // call and record nothing, consistent with `frame_stats.frames_drawn`
        // also not incrementing on those paths.
        #[cfg(feature = "frame-profiling")]
        let update_start = std::time::Instant::now();

        // ── Settings window rendering ────────────────────────────────────────
        // If this update is for the settings window, render settings directly
        // and return — no terminal state to process.
        if self.settings_window_id == Some(window_id) {
            // OS dark/light preference (used to pick the auto-mode theme slug).
            // The settings window has no `PerWindowState`, so source it from
            // the owning terminal window's stable `os_dark_mode`. We must NOT
            // read it back from `ctx.global_style().visuals.dark_mode`, because
            // we overwrite the visuals below with a palette-derived `dark_mode`
            // — reading that back next frame would be self-referential.
            let os_dark = self
                .settings_owner
                .and_then(|owner| self.windows.get(&owner))
                .map_or_else(|| ctx.global_style().visuals.dark_mode, |w| w.os_dark_mode);

            // Apply the centralized themed chrome `Visuals` to the settings
            // window's own egui context. The settings window is a separate OS
            // window with its own `ctx`, and this branch returns before the
            // per-frame style hook in the terminal render path runs — so
            // without this the settings window stays on egui's default visuals
            // and ignores the active theme + profile (112.7 follow-up).
            //
            // Style from the *draft* (unsaved) theme so selecting a new theme
            // in the picker repaints the settings window live, instead of
            // staying on the committed theme until Apply + re-open.
            let active_slug = self.settings_modal.draft_active_theme_slug(os_dark);
            let theme = freminal_common::themes::by_slug(&active_slug)
                .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
            let visuals = crate::gui::chrome_style::build_visuals(
                &self.gui_theme,
                theme,
                self.config.ui.background_opacity,
            );
            let gui_theme = self.gui_theme;
            ctx.global_style_mut(|style| {
                style.visuals = visuals;
                crate::gui::chrome_style::apply_chrome_spacing(style, &gui_theme);
            });

            // Sync discovered layout list into the modal each frame so the
            // Startup tab always shows fresh data.
            self.settings_modal.discovered_layouts = self.discovered_layouts.clone();
            let settings_action = self.settings_modal.show_standalone(ctx, os_dark);
            self.handle_settings_action(&settings_action, handle, window_id);

            // Track the settings window's current geometry so we can restore
            // it the next time it is opened.  We query the windowing layer
            // directly rather than `ctx.input().viewport()` because the
            // latter only populates `inner_rect` / `outer_rect` after a
            // Resized / Moved event reaches the window's egui context, which
            // is not guaranteed on the first frame of a freshly created
            // window on every platform.  The windowing layer always tracks
            // live geometry from winit events + direct window queries.
            if let Some(geom) = handle.window_geometry(window_id) {
                if let Some(size) = geom.size {
                    self.window_state.settings.size = Some(<[u32; 2]>::from(size));
                }
                if let Some(pos) = geom.position {
                    self.window_state.settings.position = Some(<[i32; 2]>::from(pos));
                }
            }

            // If the modal closed (Cancel or Apply), close the OS window.
            if !self.settings_modal.is_open {
                // Drop the live chrome preview override: the session is over.
                // On Apply the committed theme now flows via the snapshot; on
                // Cancel the RevertTheme broadcast restored it. Clearing also
                // re-enables per-window Auto-mode theming, which a pinned global
                // override cannot represent. The follow-up repaints scheduled by
                // the Apply / Revert dispatch cover the snapshot catch-up.
                self.preview_theme = None;
                self.persist_window_state();
                self.settings_window_id = None;
                self.settings_owner = None;
                handle.close_window(window_id);
            }
            return;
        }

        // ── Periodic session auto-save ───────────────────────────────────────
        // The background timer latches `session_save_due`; drain it here so a
        // due save runs on the terminal-window update path (the settings
        // window returned above).  Cheap no-op when not due.
        self.poll_session_autosave();

        // ── Focus or create settings window (deferred from menu/keybind) ─────
        if self.pending_focus_settings {
            self.pending_focus_settings = false;
            if let Some(sid) = self.settings_window_id {
                handle.focus_window(sid);
            }
        }
        if self.pending_settings_window && self.settings_window_id.is_none() {
            // Don't clear pending_settings_window here — cleared in on_window_created.
            // Seed inner_size and position from the last-known geometry so the
            // window reopens where the user left it (both within a session and
            // across sessions via window_state.toml).  Falls back to the 600x500
            // default on first open / missing state.
            let settings_geom = self.window_state.settings;
            let inner_size = settings_geom.size.map_or((600_u32, 500_u32), <_>::from);
            let position = settings_geom.position.map(<_>::from);
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal Settings".to_owned(),
                inner_size: Some(inner_size),
                position,
                transparent: false,
                icon: self.icon.clone(),
                app_id: Some("freminal-settings".into()),
            });
        }

        // Remove per-window state for the duration of this frame.
        // All other windows remain in the map, so shader/bg propagation
        // to "other windows" simply iterates self.windows.
        let Some(mut win) = self.windows.remove(&window_id) else {
            // This window has no PerWindowState — normally a transient state
            // during teardown, but if the only/last shell failed to spawn it
            // is permanent.  Rather than leave a blank surface, render the
            // fatal-error panel (with an Exit button) when one is set.
            if self.fatal_error.is_some() {
                self.render_fatal_error(ctx);
            }
            return;
        };

        // ── Chrome-damage (#436.3): §3.5 "before" sample + warm-up counter ───
        //
        // Sampled as early as possible in `update()` — before ANY dialog's
        // `.show(ctx)` this frame (including the toast stack's, which runs
        // after `win` is reinserted at the end of this function) — so it
        // reflects presence strictly BEFORE this frame's rendering can
        // mutate it. Compared against the "after" sample taken once
        // everything dismissible has shown (see the end of this function) to
        // catch a self-dismissal that happens DURING this frame's rendering
        // (adversarial finding 1; see `chrome_damage`'s module doc).
        let chrome_dismissible_before = self.sample_dismissible_presence(&win);
        // #436 §7 warm-up: force `Changed` for the first few frames after
        // window creation, while font atlas / layout / PanelState id-maps
        // are still settling.
        let chrome_warming_up = chrome_damage::is_chrome_warming_up(win.chrome_frames_rendered);
        win.chrome_frames_rendered = win.chrome_frames_rendered.saturating_add(1);

        // ── Drain shader/renderer errors stashed by last frame's PaintCallback ──
        // PaintCallbacks run on the render thread and can't access `self`, so
        // they stash compile/init errors in `WindowPostRenderer::last_error`.
        // Drained here every frame (71.4 bug fix): previously only ran in the
        // subsequent-window branch of `on_window_created`, which never fires
        // for the first/only window and never re-runs after window creation.
        {
            let err = {
                let mut wpr = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                wpr.last_error.take()
            };
            if let Some(msg) = err {
                self.push_error_toast("Shader error", Some(msg));
            }
        }

        // ── Spawn new window ─────────────────────────────────────────────────
        if win.pending_new_window {
            win.pending_new_window = false;
            self.spawn_new_window(handle);
        }

        // ── Apply pending window geometry from layout engine ─────────────────
        if let Some((size_opt, pos_opt)) = win.pending_geometry.take() {
            use conv2::ConvUtil as _;
            if let Some([w, h]) = size_opt {
                // u32 -> f32 via approx is always Ok for window dimensions.
                let w_f: f32 = w.approx_as().unwrap_or(f32::MAX);
                let h_f: f32 = h.approx_as().unwrap_or(f32::MAX);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(w_f, h_f)));
            }
            if let Some([x, y]) = pos_opt {
                // i32 -> f32 via approx is always Ok for screen coordinates.
                let x_f: f32 = x.approx_as().unwrap_or(0.0_f32);
                let y_f: f32 = y.approx_as().unwrap_or(0.0_f32);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(x_f, y_f)));
            }
        }

        // ── Track last known window geometry (for save_layout) ───────────────
        // Query the windowing layer directly.  See the settings-window branch
        // above for why `ctx.input().viewport()` is not reliable here.
        //
        // `chrome_size_before` is captured for the #436.3 §3.3 "window
        // resize" chrome signal: compared against `win.last_known_size`
        // after this block updates it.
        let chrome_size_before = win.last_known_size;
        if let Some(geom) = handle.window_geometry(window_id) {
            if let Some(size) = geom.size {
                win.last_known_size = Some(<[u32; 2]>::from(size));
            }
            if let Some(pos) = geom.position {
                win.last_known_position = Some(<[i32; 2]>::from(pos));
            }
        }
        let chrome_size_changed = win.last_known_size != chrome_size_before;
        // Resize-overlay trigger (issue #433): a GENUINE OS-window resize is a
        // change between two KNOWN sizes. The first observation of a new
        // window is `None -> Some(initial)`, which `chrome_size_changed`
        // (correctly, for chrome damage) treats as a change but which must NOT
        // pop the resize overlay — otherwise the HUD flashes on every app
        // launch and every `Ctrl+Shift+N`. Requiring `Some -> Some` with
        // different values suppresses that launch/spawn false-positive.
        let window_genuinely_resized = matches!(
            (chrome_size_before, win.last_known_size),
            (Some(before), Some(after)) if before != after
        );

        // ── Deferred egui font update from standalone settings window ────────
        win.terminal_widget
            .flush_egui_fonts_if_dirty(ctx, &self.config);

        // ── Detect OS dark/light preference changes ───────────────────────────
        let current_os_dark = ctx.global_style().visuals.dark_mode;
        if current_os_dark != win.os_dark_mode {
            win.os_dark_mode = current_os_dark;

            // Always propagate the updated OS preference so DECRPM ?2031
            // reflects the new dark/light state, regardless of ThemeMode.
            for tab in win.tabs.iter() {
                if let Ok(panes) = tab.pane_tree.iter_panes() {
                    for pane in panes {
                        send_or_log!(
                            pane.input_tx,
                            InputEvent::ThemeModeUpdate(self.config.theme.mode, win.os_dark_mode,),
                            "Failed to send ThemeModeUpdate on OS change to pane"
                        );
                    }
                }
            }

            if self.config.theme.mode == ThemeMode::Auto {
                let slug = self.config.theme.active_slug(win.os_dark_mode);
                if let Some(theme) = freminal_common::themes::by_slug(slug) {
                    // Notify every pane in every tab so all PTY threads get the new palette.
                    for tab in win.tabs.iter() {
                        if let Ok(panes) = tab.pane_tree.iter_panes() {
                            for pane in panes {
                                send_or_log!(
                                    pane.input_tx,
                                    freminal_terminal_emulator::io::InputEvent::ThemeChange(theme),
                                    "Failed to send auto ThemeChange to pane"
                                );
                            }
                        }
                    }
                    rendering::update_egui_theme(
                        ctx,
                        theme,
                        self.config.ui.background_opacity,
                        &self.gui_theme,
                    );
                    // Invalidate theme cache on all panes in all tabs so the
                    // next frame forces a full vertex rebuild with the new palette.
                    for tab in win.tabs.iter_mut() {
                        if let Ok(panes) = tab.pane_tree.iter_panes_mut() {
                            for pane in panes {
                                pane.render_cache.invalidate_theme_cache();
                            }
                        }
                    }
                }
            }
        }

        // ── Shader hot-reload ─────────────────────────────────────────────────
        // When hot_reload is enabled and a shader file is configured, check the
        // file's mtime each frame and push a recompile to all panes if it changed.
        if self.config.shader.hot_reload
            && let Some(ref shader_path) = self.config.shader.path.clone()
        {
            let new_mtime = std::fs::metadata(shader_path)
                .and_then(|m| m.modified())
                .ok();
            let changed = match (new_mtime, win.shader_last_mtime) {
                (Some(new), Some(prev)) => new != prev,
                (Some(_), None) => true,
                _ => false,
            };
            if changed {
                win.shader_last_mtime = new_mtime;
                match std::fs::read_to_string(shader_path) {
                    Ok(src) => {
                        if let Ok(mut wpr) = win.window_post.lock() {
                            wpr.pending_shader = Some(Some(src.clone()));
                        }
                        // Propagate to all other windows (win is removed from map).
                        for other_win in self.windows.values() {
                            if let Ok(mut wpr) = other_win.window_post.lock() {
                                wpr.pending_shader = Some(Some(src.clone()));
                            }
                        }
                    }
                    Err(e) => {
                        error!(
                            "Shader hot-reload: failed to read '{}': {e}",
                            shader_path.display()
                        );
                    }
                }
            }
        }

        // ── Drain CommandFinishedEvent from each pane (Task 72.9) ─────────────
        // See `drain_command_finished_events` for the full description;
        // extracted as a zero-egui helper (Task 122.7). Read focus live
        // from egui rather than a per-pane cached flag: the latter is only
        // ever updated while a given pane happens to be the active one, so
        // it goes permanently stale for every other pane (regression that
        // left the visual bell overlay stuck; see `paint_bell_flash`).
        let window_focus = WindowFocus::from_bool(ctx.input(|i| i.focused));
        let tab_title_policy = self.config.tab_title.policy;
        let tab_title_separator = self.config.tab_title.separator.clone();
        drain_command_finished_events(
            &mut win.tabs,
            tab_title_policy,
            &tab_title_separator,
            &self.config.notifications,
            &self.config.bell,
            window_focus,
            &self.toasts,
        );

        // ── Poll all tabs for PTY death signals ───────────────────────────────
        // See `process_dead_panes` for the full description; extracted as a
        // zero-egui, zero-`self.windows` helper (Task 122.7). It cannot
        // itself close the window, so a `CloseWindow` outcome means the
        // caller must perform the reinsert-and-close dance that the
        // original inline code did directly.
        match process_dead_panes(&mut win, &self.recording_swap) {
            DeadPaneOutcome::Continue => {}
            DeadPaneOutcome::CloseWindow => {
                self.windows.insert(window_id, win);
                ctx.send_viewport_cmd(ViewportCommand::Close);
                return;
            }
        }

        // Load the latest snapshot from the PTY thread — no lock, single atomic load.
        let (snap, pane_scroll_offset) = {
            let Some(active_pane_ref) = win.tabs.active_tab().active_pane() else {
                warn!("update: active tab has no active pane; skipping render frame");
                // CLEANUP-436-A: `win` was moved out of `self.windows` at the
                // top of this frame (`self.windows.remove(&window_id)`); every
                // early return after that point must reinsert it (see the
                // `CannotCloseLastPane` arm above, which does). Skipping the
                // reinsert here permanently orphans this window's
                // `PerWindowState` — including, since #436, its chrome cache and
                // self-dismissal settle state — leaving the window rendering a
                // blank/fatal-error surface forever. Reinsert before returning.
                self.windows.insert(window_id, win);
                return;
            };
            (
                active_pane_ref.arc_swap.load_full(),
                active_pane_ref.view_state.scroll_offset,
            )
        };

        // Sync the GUI's scroll offset from the snapshot.  When new PTY output
        // arrives the PTY thread resets its offset to 0, so the snapshot will
        // carry scroll_offset = 0 even if the GUI previously sent a non-zero
        // value.  Adopting the snapshot's value keeps ViewState in sync.
        if pane_scroll_offset != snap.scroll_offset
            && let Some(p) = win.tabs.active_tab_mut().active_pane_mut()
        {
            p.view_state.scroll_offset = snap.scroll_offset;
        }

        // Apply the full palette-derived chrome `Visuals` (112.4) BEFORE any
        // chrome is drawn this frame.  The menu bar and tab bar are rendered
        // immediately below, so the style must be in place first — applying it
        // after them (as it was) left the bars styled by the *previous* frame's
        // palette, so a live theme change did not reach the menu/tab bar until
        // a later frame happened to repaint them (the "bars don't update"
        // symptom).
        //
        // Gated: only call `global_style_mut` when the inputs have changed.
        // `global_style_mut` triggers `Arc::make_mut` on the egui `Style`,
        // which clones every frame unless skipped — and `build_visuals` itself
        // is non-trivial, so the cache short-circuits the rebuild on the
        // steady-state (unchanged) path.
        //
        // Chrome styles from the live preview theme when one is active
        // (Settings theme picker), falling back to the snapshot's theme at
        // steady state.  The preview override makes chrome re-theme immediately
        // and deterministically — it does not depend on the GUI happening to
        // read the post-`ThemeChange` snapshot (a race that left the
        // background/chrome stale until a mouseover repaint).
        let bg_opacity = self.config.ui.background_opacity;
        // Hoisted out of the block below (rather than a plain `let` inside
        // it) so the #436.3 chrome-damage signal computed further down can
        // read this frame's style-change verdict too.
        let chrome_style_changed;
        {
            let gui_theme = self.gui_theme;
            let chrome_theme = self.preview_theme.unwrap_or(snap.theme);
            chrome_style_changed = match win.style_cache {
                Some((prev_theme, prev_opacity, prev_gui_theme)) => {
                    !std::ptr::eq(prev_theme, chrome_theme)
                        || prev_opacity.to_bits() != bg_opacity.to_bits()
                        || prev_gui_theme != gui_theme
                }
                None => true,
            };
            if chrome_style_changed {
                let visuals =
                    crate::gui::chrome_style::build_visuals(&gui_theme, chrome_theme, bg_opacity);
                ctx.global_style_mut(|style| {
                    style.visuals = visuals;
                    crate::gui::chrome_style::apply_chrome_spacing(style, &gui_theme);
                });
                win.style_cache = Some((chrome_theme, bg_opacity, gui_theme));
            }
        }

        // ── #436.4b: FULL vs REPLAY chrome construction ──────────────────────
        //
        // On `ChromeMode::Full` the root Ui, menu bar, and tab bar are built
        // exactly as before, and `CentralPanel` reserves the remaining space
        // for `central_body` below. On `ChromeMode::Replay` the windowing
        // layer has already proven chrome (including window size) is
        // unchanged since the last FULL frame, so none of that is rebuilt —
        // `central_body` runs directly against a `Ui` constructed at the
        // cached content rect instead, in the SAME background layer chrome
        // uses (so the terminal band's shapes land exactly where `run_frame`
        // expects them). `any_menu_open` (read inside `central_body` to
        // compute `ui_overlay_open`) is `false` on Replay: with no menu bar
        // built, no menu can be open.
        let (any_menu_open, chrome_root_ui): (bool, Option<egui::Ui>) = if chrome_mode
            == freminal_windowing::ChromeMode::Full
        {
            // Create a root Ui covering the full available area.  Panels
            // reserve space from this Ui via `show` (the non-deprecated
            // API; `show_inside` was renamed to `show` in egui 0.35).
            let mut root_ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::new("freminal_root"),
                egui::UiBuilder::default(),
            );

            // #436.8: menu-bar / tab-bar rects, captured for the
            // region-aware pointer chrome-gate (`is_chrome_interactive_at`).
            // Only ever populated on a FULL frame — a REPLAY frame builds
            // neither panel, so `win.published`'s head rects are left
            // untouched (stale-but-still-correct: chrome hasn't moved since
            // the FULL frame that last set them, by the same invariant that
            // makes REPLAY safe at all).
            let mut head_rects: Vec<egui::Rect> = Vec::new();

            // Menu bar at the top of the window.
            let menu_open = if self.config.ui.hide_menu_bar {
                false
            } else {
                let menu_response = Panel::top("menu_bar").show(&mut root_ui, |ui| {
                    self.show_menu_bar(ui, &mut win, window_id)
                });
                head_rects.push(menu_response.response.rect);
                let (menu_action, menu_open) = menu_response.inner;
                self.dispatch_tab_bar_action(menu_action, &mut win);
                menu_open
            };

            // Tab bar: shown when multiple tabs are open, or when the
            // config option `tabs.show_single_tab` is enabled.
            let show_tab_bar = win.tabs.tab_count() > 1 || self.config.tabs.show_single_tab;

            if show_tab_bar {
                let panel = match self.config.tabs.position {
                    freminal_common::config::TabBarPosition::Top => Panel::top("tab_bar"),
                    freminal_common::config::TabBarPosition::Bottom => Panel::bottom("tab_bar"),
                };
                let tab_response = panel.show(&mut root_ui, |ui| self.show_tab_bar(&mut win, ui));
                head_rects.push(tab_response.response.rect);
                let tab_action = tab_response.inner;
                self.dispatch_tab_bar_action(tab_action, &mut win);
            }

            win.published.publish_chrome_head_rects(head_rects);

            (menu_open, Some(root_ui))
        } else {
            (false, None)
        };

        // Help menu → "Keybindings..." routes here.  Opens the Settings
        // Modal with the Keybindings tab preselected, or focuses the
        // existing settings window if one is already open.  Mirrors the
        // Settings menu item in `show_menu_bar`, but jumps to the
        // Keybindings tab instead of the default Font tab. Independent of
        // `chrome_mode`: it only mutates modal-open state (no painting), so
        // it runs every frame regardless of whether chrome was rebuilt.
        if self.pending_open_keybindings {
            self.pending_open_keybindings = false;
            if self.settings_window_id.is_some() {
                self.pending_focus_settings = true;
                self.settings_modal
                    .set_active_tab(crate::gui::settings::SettingsTab::Keybindings);
            } else if !self.settings_modal.is_open && !self.pending_settings_window {
                let families = win.terminal_widget.monospace_families();
                self.settings_modal.open_to_tab(
                    &self.config,
                    families,
                    win.os_dark_mode,
                    crate::gui::settings::SettingsTab::Keybindings,
                );
                self.settings_modal
                    .set_base_font_defs(win.terminal_widget.base_font_defs().clone());
                self.settings_owner = Some(window_id);
                self.pending_settings_window = true;
            }
        }

        // Copy the cached content rect (`egui::Rect` is `Copy`) out of `win`
        // BEFORE `central_body` captures `win` by mutable reference below —
        // reading `win.published`'s cached central rect after that point
        // (e.g. inside the REPLAY arm further down) would conflict with the
        // closure's borrow. Only actually used on a REPLAY frame; harmless
        // (and cheap) to compute unconditionally otherwise. Falls back to
        // egui's own content rect in the unreachable case described where it
        // is used.
        let cached_central_rect_for_replay = win
            .published
            .cached_central_rect()
            .unwrap_or_else(|| ctx.input(egui::InputState::content_rect));

        // Task 121 frame-profiling harness: `central_body` (below) hands its
        // two measured phase durations OUT through these captured locals
        // rather than writing straight into `win.frame_stats` and flushing
        // itself. This is the defect-2 fix's other half: the
        // `frame_damage_full`/`frame_damage_partial` counters need to be
        // recorded from the FINAL, post-`compose_with_chrome_damage` value
        // of `win.pending_frame_damage`, which is not known until after
        // `central_body` returns (on EITHER the FULL or REPLAY branch below)
        // -- so the accumulation-into-`FrameStats` and the `tracing::debug!`
        // flush both moved to a single recording point after that
        // composition, near the end of this function. `central_body` is
        // `FnMut`, invoked exactly once per frame (on whichever branch is
        // taken), so a plain `Duration` local -- mutably captured, written
        // once inside the closure, read once after it returns -- is
        // sufficient; no `Cell`/`RefCell`/extra `PerWindowState` field is
        // needed.
        #[cfg(feature = "frame-profiling")]
        let mut phase_orchestration_out = std::time::Duration::ZERO;
        #[cfg(feature = "frame-profiling")]
        let mut phase_panes_out = std::time::Duration::ZERO;

        // The terminal band + (on Full only) chrome dialogs/overlays. Shared
        // between the FULL path (called via `CentralPanel::show`, below) and
        // the REPLAY path (called directly against a `Ui` built at the
        // cached content rect) so the band's rendering logic — the per-pane
        // loop, borders, broadcast label, band-range capture, chrome-damage
        // signal staging, and repaint scheduling — is defined exactly once.
        let mut central_body = |ui: &mut egui::Ui| {
            // Task 121 frame-profiling harness: wall-clock start of this
            // whole closure. At the end of the closure this is used, minus
            // the accumulated per-pane `phase_panes_this_frame`, to derive
            // `phase_orchestration` -- freminal's own orchestration overhead as
            // distinct from time spent inside `terminal_widget.show()`.
            // Not split into narrower named sub-phases because the
            // bookkeeping this measures (window-manipulation drain, OSC
            // routing, border drag-sensor rebuild, the per-pane
            // resize-debounce/scroll-sync checks interleaved in the loop
            // below, and the post-loop focus-follows-mouse hit-testing) is
            // itself interleaved with the per-pane loop, not a single
            // contiguous block that could be timed in isolation.
            #[cfg(feature = "frame-profiling")]
            let central_body_start = std::time::Instant::now();

            // Synchronise font metrics with the current display scale *before*
            // reading `cell_size()`.  Without this, the first frame after a DPI
            // change would use stale pixel metrics for the resize calculation.
            let ppp = ctx.pixels_per_point();
            let ppp_changed = win.terminal_widget.sync_pixels_per_point(ppp);

            // Synchronise font zoom for the active tab.  Each tab has its own
            // zoom_delta and the font manager only knows one size at a time.
            // This check fires on every frame but is a single float comparison
            // when no change is needed.
            let effective = win
                .tabs
                .active_tab()
                .active_pane()
                .map_or(self.config.font.size, |p| {
                    p.view_state.effective_font_size(self.config.font.size)
                });
            let zoom_changed = win.terminal_widget.apply_font_zoom(effective);

            // When pixels-per-point or font zoom changes, every pane's GL
            // atlas and cached content must be invalidated so glyphs are
            // re-rasterised at the new size.
            if ppp_changed || zoom_changed {
                win.invalidate_all_pane_atlases();
            }

            // Compute char size once — shared across all panes since all panes
            // use the same font at the same size.
            // `cell_size()` returns integer pixel dimensions (physical) from swash
            // font metrics.  egui's coordinate system uses logical points, so we
            // convert with `pixels_per_point` when doing layout math.
            let (cell_w_u, cell_height_u) = win.terminal_widget.cell_size();
            let font_width = usize::value_from(cell_w_u).unwrap_or(0);
            let font_height = usize::value_from(cell_height_u).unwrap_or(0);
            let logical_char_w = f32::approx_from(cell_w_u).unwrap_or(0.0) / ppp;
            let logical_char_h = f32::approx_from(cell_height_u).unwrap_or(0.0) / ppp;

            // Command-block gutter inset, in logical points.  This is reserved
            // on the left edge of every pane's content rect when the gutter is
            // enabled.  It is subtracted from the available width BEFORE the
            // column count is computed (below) so the column count reported to
            // the PTY matches the rendered cell-grid width — the renderer
            // shifts its terminal rect right by the same inset.  Zero when the
            // feature is disabled or the gutter is set to `Off`.
            let gutter_inset_logical = if self.config.command_blocks.enabled {
                self.config.command_blocks.gutter.total_inset_px() / ppp
            } else {
                0.0
            };
            // Task 121 spike: publish the LOGICAL inset for
            // `App::pointer_motion_needs_repaint`, which runs outside a frame
            // and so has no `ppp` of its own. See the field's doc for why this
            // beats assuming `ppp >= 1.0`.
            win.published
                .publish_cached_gutter_inset_logical(gutter_inset_logical);

            // Read live from egui rather than a per-pane cached flag: the
            // latter is only ever updated while a given pane happens to be
            // the active one, so it goes permanently stale for every other
            // pane (regression that left the visual bell overlay stuck).
            let window_focused = ui.input(|i| i.focused);
            // #436.3 §3.3 "Window focus change" chrome signal.
            let chrome_focus_changed = window_focused != win.prev_window_focused;
            win.prev_window_focused = window_focused;

            // Drain window commands for ALL tabs and ALL panes within each
            // tab, then route the OSC 9/777, OSC 52, and OSC 99 events that
            // drain collected (Task 122.8). See
            // `drain_window_manipulation_commands`'s doc for the
            // active/non-active discard-rule contract this preserves.
            let window_manipulation_events = drain_window_manipulation_commands(
                ui,
                &mut win.tabs,
                font_width,
                font_height,
                WindowFocus::from_bool(window_focused),
                &self.config,
            );
            self.route_window_manipulation_events(
                ui,
                WindowFocus::from_bool(window_focused),
                &window_manipulation_events,
            );

            // ── Multi-pane rendering loop ────────────────────────────
            //
            // Compute layout rects for every leaf pane in the active tab's
            // pane tree, then render each one into its allocated rect.
            // Collect deferred key actions from all panes for dispatch after
            // the loop.

            let available_rect = ui.available_rect_before_wrap();

            // #436.4b: cache the content rect so a later REPLAY frame can
            // reconstruct an equivalent `Ui` without rebuilding the menu
            // bar / tab bar / `CentralPanel` chrome that produced it.
            // Idempotent on a REPLAY frame itself (`available_rect` is then
            // already exactly the cached value).
            win.published.publish_cached_central_rect(available_rect);

            let active_pane_id = win.tabs.active_tab().active_pane;
            let zoomed_pane = win.tabs.active_tab().zoomed_pane;
            let has_multiple_panes = win.tabs.active_tab().pane_tree.pane_count().unwrap_or(1) > 1;

            // Re-anchor the cursor blink phase when the active pane changes —
            // by a pane switch within the tab OR a tab switch (both change the
            // "which pane is active-and-visible" key). This makes the newly
            // active pane's cursor appear immediately instead of inheriting the
            // global blink cycle's current half (the cursor-appear lag). The
            // flag is captured on that pane's next render, when the egui input
            // clock is available. Only reset on an actual change so we don't
            // re-anchor (and cause a spurious extra blink) every frame.
            let active_pane_key = (win.tabs.active_tab().id, active_pane_id);
            let active_pane_changed = win.previous_active_pane_key != Some(active_pane_key);
            if active_pane_changed {
                if let Some(pane) = win.tabs.active_tab_mut().pane_tree.find_mut(active_pane_id) {
                    pane.view_state.cursor_blink_reset_pending = true;
                }
                win.previous_active_pane_key = Some(active_pane_key);
            }

            // Broadcast input (Task 74): when the active tab has broadcast
            // enabled, collect the (pane id, input sender) of every leaf pane
            // up front. Senders are cheap to clone. The active pane's render
            // call mirrors its keyboard input to every *other* pane in this
            // list. Empty when broadcast is off (the common case).
            let broadcast_senders: Vec<(panes::PaneId, crossbeam_channel::Sender<InputEvent>)> =
                if win.tabs.active_tab().broadcast_input {
                    win.tabs
                        .active_tab()
                        .pane_tree
                        .iter_panes()
                        .map(|panes| {
                            panes
                                .into_iter()
                                .map(|p| (p.id, p.input_tx.clone()))
                                .collect()
                        })
                        .unwrap_or_default()
                } else {
                    Vec::new()
                };

            // When a pane is zoomed, render only that pane at full size.
            // Borders are hidden during zoom since there is only one visible pane.
            let (pane_layout, border_width) = if let Some(zoomed_id) = zoomed_pane {
                (vec![(zoomed_id, available_rect)], 0.0)
            } else {
                // Width of the border drawn between adjacent panes (logical pixels).
                let bw: f32 = if has_multiple_panes { 1.0 } else { 0.0 };
                // `PaneTree::layout` takes/returns the toolkit-neutral
                // `geometry::Rect`; `pane_layout` below is consumed by many
                // egui-painting loops further down, so convert once here
                // (eagerly) rather than at each of those use sites.
                let layout = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .layout(geometry_interop::rect_from_egui(available_rect))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(id, r)| (id, geometry_interop::rect_to_egui(r)))
                    .collect();
                (layout, bw)
            };

            let mut all_deferred_actions = Vec::new();

            // ── #436.4b: chrome dialogs/overlays — FULL only ──────────────
            //
            // These are all cached TAIL chrome (each uses its own
            // `egui::Window`/`Area`, a distinct layer from the terminal
            // band's `LayerId::background()`). A REPLAY frame is only ever
            // entered when the PREVIOUS frame proved `ui_overlay_open` was
            // `false` (any dialog open forces `ChromeDamage::Changed` every
            // frame it is — see `ChromeSignals::any_fired`) and no chrome
            // input landed this frame, so by construction none of these can
            // be open (or becoming open) on a REPLAY frame. Skipping their
            // `.show()` calls is therefore safe; running them would be
            // wasted work whose freshly-painted shapes `run_frame` would
            // discard anyway (REPLAY reuses the cached tail primitives, not
            // this frame's own tail shapes).
            if chrome_mode == freminal_windowing::ChromeMode::Full {
                // Floating "Save Layout" name-entry prompt.  Shown whenever the
                // user clicked "Save Layout" in the Layouts menu.  Returns true
                // exactly once (the frame the user confirms), at which point we
                // enqueue the SaveLayout action for dispatch.
                if self.show_save_layout_prompt(ctx) {
                    all_deferred_actions.push(freminal_common::keybindings::KeyAction::SaveLayout);
                }

                // Smart paste guard confirm dialog (Task 77).  Shown whenever a
                // flagged paste is pending for this window.  On confirm, the
                // resolved (possibly edited) payload is sent to the active pane;
                // on cancel the paste is discarded.
                match win.paste_dialog.show(ctx) {
                    super::paste_guard::PasteDialogOutcome::Paste { payload, target } => {
                        // Route to the pane captured when the dialog opened, not
                        // the currently-active pane: focus-follows-mouse can change
                        // the active pane when the cursor moves onto the dialog
                        // buttons (Task 106 bug).
                        Self::send_paste_to_target(&mut win, target, payload);
                    }
                    super::paste_guard::PasteDialogOutcome::Cancelled => {
                        self.route_freminal_toast(
                            freminal_common::config::FreminalToastCategory::PasteBlocked,
                            crate::gui::toast::ToastKind::Warning,
                            "Paste blocked",
                            None,
                            crate::gui::toast::ToastPlacement::WINDOW_CENTERED,
                        );
                    }
                    super::paste_guard::PasteDialogOutcome::Idle => {}
                }

                // Broadcast-input confirm dialog (Task 74.5).  Shown when the user
                // tried to enable broadcast and `[tabs] confirm_broadcast` is set.
                // On confirm, broadcast is enabled on the dialog's target tab.
                match win.broadcast_dialog.show(ctx) {
                    super::broadcast_guard::BroadcastDialogOutcome::Confirmed(tab_id) => {
                        if let Some(tab) = win.tabs.iter_mut().find(|t| t.id == tab_id) {
                            tab.broadcast_input = true;
                            let pane_count = tab.pane_tree.iter_panes().map_or(1, |p| p.len());
                            self.push_info_toast(
                                "Broadcast input enabled",
                                Some(format!(
                                    "Keyboard input is now sent to all {pane_count} pane(s) in this tab."
                                )),
                            );
                        }
                    }
                    super::broadcast_guard::BroadcastDialogOutcome::Cancelled
                    | super::broadcast_guard::BroadcastDialogOutcome::Idle => {}
                }

                // Close-on-running-command guard dialog (Task 98).  Shown while a
                // pane / tab / window close is suspended pending confirmation.  On
                // Force Close the original close is executed with the guard
                // bypassed; on Cancel the close is abandoned.
                // A pending ForceClose key action resolves an open close-guard
                // dialog as Force Close; harmless no-op when nothing is open.
                let force_close_requested = std::mem::take(&mut win.pending_force_close);
                if let Some(scope) = win.close_dialog.scope() {
                    let outcome = if force_close_requested {
                        win.close_dialog.force_close_now();
                        super::close_guard::CloseDialogOutcome::ForceClose
                    } else {
                        win.close_dialog.show(ctx)
                    };
                    match outcome {
                        super::close_guard::CloseDialogOutcome::ForceClose => match scope {
                            super::close_guard::CloseScope::Pane => {
                                Self::close_focused_pane(ui, &mut win);
                            }
                            super::close_guard::CloseScope::Tab(index) => {
                                win.close_tab(index);
                            }
                            super::close_guard::CloseScope::Window => {
                                // Mark this window as user-confirmed so the
                                // on_close_requested guard lets the resulting
                                // ViewportCommand::Close through without re-prompting.
                                self.force_close_windows.insert(window_id);
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            super::close_guard::CloseScope::WindowUnsavedSettings => {
                                // User chose to discard the unsaved settings
                                // edits. Close the settings OS window directly —
                                // `handle` is available right here, unlike in
                                // `on_close_requested` — then re-issue this
                                // window's close. Clearing `settings_owner` here
                                // (rather than a separate "confirmed" flag) is
                                // what makes the retry's `on_close_requested`
                                // call see `is_owner == false` and skip the
                                // guard without re-prompting (issue #401).
                                self.settings_modal.is_open = false;
                                self.settings_owner = None;
                                if let Some(sid) = self.settings_window_id.take() {
                                    handle.close_window(sid);
                                }
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        },
                        super::close_guard::CloseDialogOutcome::Cancelled
                        | super::close_guard::CloseDialogOutcome::Idle => {}
                    }
                }

                // Floating "About Freminal" dialog.  Shown whenever the user
                // clicked "About Freminal" in the Help menu.  Self-dismissing
                // via its own Close button or title-bar X.
                self.show_about_window(ctx);

                // First-run welcome overlay (subtask 71.20).  Opened on first
                // launch or from Help -> Show Welcome; persists
                // `first_run_complete = true` on dismissal.
                self.show_welcome_overlay(ctx);
            }

            // Drain pending menu actions (Edit menu clicks: Copy, Paste,
            // Select All, Find...).  These were queued during
            // `show_menu_bar` above, which does not have mutable access to
            // the active pane's ViewState / input_tx.  Menu-local actions
            // (Copy, Paste, Select All) are applied directly to the active
            // pane; others are routed through the deferred-action pipeline.
            for action in std::mem::take(&mut win.pending_menu_actions) {
                Self::dispatch_menu_action(&mut win, action, &mut all_deferred_actions);
            }

            // Track repaint needs across all panes.
            let mut shortest_repaint_delay: Option<std::time::Duration> = None;

            // Tab rename is treated as an overlay: while the inline rename
            // TextEdit is active, the terminal widget must release keyboard
            // focus and stop consuming pointer events, or keystrokes would
            // be forwarded to the PTY instead of the edit buffer.
            let ui_overlay_open = any_menu_open
                || self.pending_save_layout.is_some()
                || self.about_window_open
                || self.welcome.is_open()
                || win.renaming_tab.is_some()
                || win.paste_dialog.is_open()
                || win.broadcast_dialog.is_open()
                || win.close_dialog.is_open();

            // ── Pane border drag-to-resize ───────────────────────────
            //
            // Before rendering panes, place invisible drag sensors on each
            // split border. This must happen before the per-pane
            // `scope_builder` calls so that pointer events on the border
            // are consumed here instead of reaching the terminal widgets.
            // Whether a split-border sensor owns the cursor icon this frame.
            // Set from the very same condition that applies the resize cursor
            // below, so the gate and the write can never disagree (#462).
            let mut border_owns_cursor = false;
            if has_multiple_panes && zoomed_pane.is_none() && !ui_overlay_open {
                let borders = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(
                        geometry_interop::rect_from_egui(available_rect),
                        active_pane_id,
                    )
                    .unwrap_or_default();

                // Half-width of the invisible drag sensor zone (pixels
                // on each side of the 1px border line).
                let sensor_half: f32 = 3.0;

                // #436.8: split-border drag-sensor rects, rebuilt fresh every
                // frame this branch runs, for the region-aware pointer
                // chrome-gate (`is_chrome_interactive_at`).
                let mut border_rects: Vec<egui::Rect> = Vec::with_capacity(borders.len());

                for (border_idx, border) in borders.iter().enumerate() {
                    // Expand the thin 1px border rect into a wider sensor rect.
                    let sensor_rect = match border.direction {
                        panes::SplitDirection::Horizontal => {
                            // Vertical divider — expand horizontally.
                            let cx = border.rect.center().x;
                            egui::Rect::from_min_max(
                                egui::pos2(cx - sensor_half, border.rect.min.y),
                                egui::pos2(cx + sensor_half, border.rect.max.y),
                            )
                        }
                        panes::SplitDirection::Vertical => {
                            // Horizontal divider — expand vertically.
                            let cy = border.rect.center().y;
                            egui::Rect::from_min_max(
                                egui::pos2(border.rect.min.x, cy - sensor_half),
                                egui::pos2(border.rect.max.x, cy + sensor_half),
                            )
                        }
                    };

                    border_rects.push(sensor_rect);

                    let sensor_id = ui.id().with("pane_border_sensor").with(border_idx);
                    let response =
                        ui.interact(sensor_rect, sensor_id, egui::Sense::click_and_drag());

                    // Change cursor when hovering or dragging a border.
                    //
                    // `dragged()` matters as much as `hovered()`: the sensor
                    // rects are built from `borders`, computed at the top of
                    // this frame and therefore one frame behind the divider
                    // that `resize_split` is currently moving. During a drag
                    // the pointer routinely runs ahead of the stale rect, so
                    // a hover-only test would drop out intermittently and let
                    // the pane underneath reclaim the icon -- the cursor
                    // flickering between the resize arrow and the normal
                    // pointer while dragging.
                    if response.hovered() || response.dragged() {
                        let cursor = match border.direction {
                            panes::SplitDirection::Horizontal => egui::CursorIcon::ResizeHorizontal,
                            panes::SplitDirection::Vertical => egui::CursorIcon::ResizeVertical,
                        };
                        ctx.set_cursor_icon(cursor);
                        border_owns_cursor = true;
                    }

                    // On drag start, record which border we're resizing.
                    if response.drag_started() {
                        win.border_drag = Some(PaneBorderDrag {
                            target_pane: border.first_child_pane,
                            direction: border.direction,
                            parent_extent: border.parent_extent,
                        });
                    }

                    // While dragging, convert pixel delta to ratio delta.
                    if response.dragged()
                        && let Some(drag) = &win.border_drag
                    {
                        let delta_px = match drag.direction {
                            panes::SplitDirection::Horizontal => response.drag_delta().x,
                            panes::SplitDirection::Vertical => response.drag_delta().y,
                        };

                        // Convert pixel delta to ratio delta based on
                        // the dragged split parent's extent along the split axis.
                        let total_px = drag.parent_extent;

                        if total_px > 0.0 {
                            let delta_ratio = delta_px / total_px;
                            if let Err(e) = win.tabs.active_tab_mut().pane_tree.resize_split(
                                drag.target_pane,
                                drag.direction,
                                delta_ratio,
                            ) {
                                debug!("Border resize failed: {e}");
                            }
                        }
                    }

                    // Clear drag state when drag ends.
                    if response.drag_stopped() {
                        // On release, focus the pane the pointer ends in (focus
                        // was frozen during the drag, issue #453). Use the
                        // release pointer position; fall back to leaving focus
                        // unchanged if it isn't over any pane.
                        if let Some(pos) = ui.ctx().pointer_hover_pos()
                            && let Some(under_id) = panes::pane_at_pos(
                                &pane_layout
                                    .iter()
                                    .map(|(id, r)| (*id, geometry_interop::rect_from_egui(*r)))
                                    .collect::<Vec<_>>(),
                                geometry_interop::point_from_egui(pos),
                            )
                        {
                            let tab = win.tabs.active_tab_mut();
                            if tab.active_pane != under_id {
                                let old_active = tab.active_pane;
                                if let Some(old_pane) = tab.pane_tree.find(old_active)
                                    && let Err(e) =
                                        old_pane.input_tx.send(InputEvent::FocusChange(false))
                                {
                                    error!(
                                        "Failed to send FocusChange(false) to pane {old_active}: {e}"
                                    );
                                }
                                tab.active_pane = under_id;
                                if let Some(new_pane) = tab.pane_tree.find(under_id)
                                    && let Err(e) =
                                        new_pane.input_tx.send(InputEvent::FocusChange(true))
                                {
                                    error!(
                                        "Failed to send FocusChange(true) to pane {under_id}: {e}"
                                    );
                                }
                            }
                        }
                        win.border_drag = None;
                        // The divider has just moved to its final position,
                        // but this frame's sensor rects were built before that
                        // move. Force one more frame so hover is re-evaluated
                        // against the settled layout, otherwise the resize
                        // cursor drops to the default arrow on mouse-up even
                        // though the pointer is still over the divider (#462).
                        ctx.request_repaint();
                    }
                }

                win.published.publish_chrome_border_rects(border_rects);
            } else {
                // No sensors built this frame (single pane / zoomed / overlay
                // open): clear any stale rects from a since-changed layout so
                // they can't keep classifying terminal content as chrome
                // (#436.8).
                win.published.clear_chrome_border_rects();
            }

            // Defensive: a border drag can only be in progress while the primary
            // button is held. If the button is up but `border_drag` is still set,
            // the drag-sensor loop missed its `drag_stopped()` transition (e.g. an
            // overlay opened mid-drag and gated the sensor block off). Clear it so a
            // stuck value can't permanently freeze input via `border_drag_active`
            // (issue #453 review). This runs every frame, independent of
            // `ui_overlay_open`/`has_multiple_panes`/`zoomed_pane`, so it can never
            // be gated off the same way the sensor block above can be.
            if win.border_drag.is_some() && !ui.ctx().input(|i| i.pointer.primary_down()) {
                win.border_drag = None;
            }

            // Whether a pane-border drag-to-resize is currently in progress.
            // Read into a local now (after the drag-sensor block above has
            // had a chance to start/stop it this frame) so the per-pane loop
            // below can pass it to `show()` without holding a borrow of
            // `win.border_drag` across the mutable borrow of
            // `win.terminal_widget` (issue #453).
            let border_drag_active = win.border_drag.is_some();

            // Whether a split-border sensor owns the cursor icon this frame.
            //
            // Those sensors are wider than the 1px border they straddle, so
            // the pointer sits geometrically inside an adjacent pane while
            // logically over chrome. Panes must abstain from writing
            // `output.cursor_icon` there, or the resize arrow survives only in
            // the hairline gap between the two pane rects.
            //
            // Driven by the same `hovered() || dragged()` test that actually
            // applies the cursor, rather than an independent hit-test of the
            // published rects: those rects are a frame behind the divider
            // during a drag, so a separate test would disagree with the write
            // exactly when it matters. `border_drag_active` is folded in so an
            // in-flight drag keeps the cursor even on a frame where the
            // stale sensor rect has fallen behind the pointer entirely (#462).
            let split_border_hover = if border_owns_cursor || border_drag_active {
                SplitBorderHover::Over
            } else {
                SplitBorderHover::Clear
            };

            // ── Terminal band: shape-index range capture (#436.2a, range
            // exposed via `App::take_terminal_band_range` as of #436.4a) ───
            //
            // Everything from here through the broadcast label that paints
            // via `ui` — pane content (GL callbacks), pane borders, and the
            // per-pane decorations — lands in the SAME `LayerId::background()`
            // layer chrome (menu bar, tab bar) already paints into, and is
            // therefore captured in the shape-index range below. (Per-pane
            // pop-ups reached from inside this region — the context menu,
            // command-history palette, and search bar — deliberately use
            // their own `Order::Foreground` `egui::Area`s, so they live in a
            // different layer and are correctly excluded from the captured
            // background range.) An earlier version of this subtask routed
            // the band into a dedicated second `Order::Background` layer
            // instead, but that
            // trips egui 0.35's cross-layer hit-test "hidden" rule
            // (`hit_test.rs:145`): a widget is hidden from hover/click/drag
            // if a later widget on a DIFFERENT layer contains its rect, and
            // two untracked same-`Order` layers tie-break by hash iteration
            // order — nondeterministically hiding every `ui.interact()`
            // widget in the band (e.g. the command-block gutter hover
            // highlight). Staying in the shared background layer keeps both
            // paint topology and hit-test topology identical to `main`; we
            // instead remember where the band's shapes start and end within
            // that single `PaintList` and hand back that `[start, end)` range
            // for `App::take_terminal_band_range`, which `run_frame` uses to
            // slice `full_output.shapes` into head/band/tail and paint each
            // separately (#436.4a).
            //
            // Capture point justification: nothing between `available_rect`
            // above and here paints into the background layer. The
            // intervening code only reads window-manipulation state, shows
            // dialogs (save-layout prompt, paste/broadcast/close guards,
            // about window, welcome overlay — all `egui::Window`, which is
            // backed by an `Area` with its own distinct `LayerId`, never
            // `LayerId::background()`), dispatches queued menu actions (no
            // painting), and registers pane-border drag sensors via
            // `ui.interact()` (which does not append any shape). So the
            // background layer's `PaintList` has not grown since the menu
            // bar / tab bar chrome painted (before this `CentralPanel`
            // closure began); capturing the count here bounds the range to
            // exactly the band.
            let band_shape_start = ctx.graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            // ── Pre-clear the window post-processing FBO ──────────
            //
            // When a user GLSL shader is active (or about to become active),
            // all panes render into a shared window FBO.  We clear it once
            // per frame here, before any pane draws into it, so stale content
            // from the previous frame does not bleed through.
            //
            // We also schedule the pre-clear when `pending_shader` is set so
            // that the very first frame after a shader is enabled already has
            // the FBO ready for pane callbacks.  The `ensure_fbo` call inside
            // the callback creates the FBO on-demand if it doesn't exist yet.
            {
                let wpr_guard = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let wpr_active = wpr_guard.is_active();
                let shader_activation_pending = wpr_guard.pending_shader.is_some();
                drop(wpr_guard);

                if wpr_active || shader_activation_pending {
                    let wpr_for_clear = Arc::clone(&win.window_post);
                    ui.painter().add(egui::PaintCallback {
                        rect: available_rect,
                        callback: Arc::new(CallbackFn::new(move |info, painter| {
                            let gl = &Gl::real(painter.gl());
                            let vp = info.viewport_in_pixels();
                            let mut wpr = wpr_for_clear
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                            if let Some(fbo) = wpr.fbo() {
                                unsafe {
                                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
                                    gl.clear_color(0.0, 0.0, 0.0, 0.0);
                                    gl.clear(glow::COLOR_BUFFER_BIT);
                                    // Restore egui's FBO.
                                    gl.bind_framebuffer(
                                        glow::FRAMEBUFFER,
                                        painter.intermediate_fbo(),
                                    );
                                }
                            }
                        })),
                    });
                }
            }

            // Clone the per-window partial-present flag once, before the pane
            // loop, so each pane's `show()` can pass it into its PaintCallback
            // without re-borrowing `win` while `win` is mutably borrowed in
            // the loop (#435).
            let present_is_partial_for_panes = std::sync::Arc::clone(&win.present_is_partial);

            // Resize overlay (issue #433): whether any pane's char grid
            // changed this frame (the debounced-resize check below). Only a
            // trigger — the displayed dimensions are recomputed from the
            // whole window content area after the loop, where `win.tabs` is
            // no longer borrowed. Captured here rather than written straight
            // to `win.published`'s resize-overlay state because the
            // per-pane loop mutably borrows `win.tabs`.
            let mut grid_size_changed = false;

            // Task 121 frame-profiling harness: cumulative time spent this
            // frame inside `terminal_widget.show()` across every pane in
            // `pane_layout` (accumulated inside the loop below).
            #[cfg(feature = "frame-profiling")]
            let mut phase_panes_this_frame = std::time::Duration::ZERO;

            // Subtask 122.15: clear last frame's published terminal-rect
            // origins before the per-pane loop republishes one entry per
            // live pane below. Panes come and go (split/close), so this
            // must happen unconditionally every frame rather than only on
            // some branch — otherwise a closed pane's origin would linger
            // in `win.published` forever.
            win.published.clear_pane_terminal_origins();

            for (pane_id, pane_rect) in &pane_layout {
                // Shrink the pane rect slightly to leave room for borders.
                // Each pane edge that is interior (shared with another pane)
                // gives up half the border width so the total gap equals
                // `border_width`.
                let content_rect = if has_multiple_panes {
                    let half = border_width / 2.0;
                    let shrink_left = if pane_rect.min.x > available_rect.min.x {
                        half
                    } else {
                        0.0
                    };
                    let shrink_right = if pane_rect.max.x < available_rect.max.x {
                        half
                    } else {
                        0.0
                    };
                    let shrink_top = if pane_rect.min.y > available_rect.min.y {
                        half
                    } else {
                        0.0
                    };
                    let shrink_bottom = if pane_rect.max.y < available_rect.max.y {
                        half
                    } else {
                        0.0
                    };
                    egui::Rect::from_min_max(
                        egui::pos2(pane_rect.min.x + shrink_left, pane_rect.min.y + shrink_top),
                        egui::pos2(
                            pane_rect.max.x - shrink_right,
                            pane_rect.max.y - shrink_bottom,
                        ),
                    )
                } else {
                    *pane_rect
                };

                // Per-pane character dimensions from this pane's content rect.
                // The gutter inset is removed from the available width first so
                // the column count matches the rendered cell grid (the widget
                // shifts its terminal rect right by the same inset).
                let pane_content_width = (content_rect.width() - gutter_inset_logical).max(0.0);
                let pane_width_chars = (pane_content_width / logical_char_w)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to calculate pane width chars: {e}");
                        10
                    });
                let pane_height_chars = (content_rect.height() / logical_char_h)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to calculate pane height chars: {e}");
                        10
                    })
                    .max(1);

                // Look up the pane mutably for resize + render.
                let pane_id = *pane_id;
                let tab = win.tabs.active_tab_mut();
                let Some(pane) = tab.pane_tree.find_mut(pane_id) else {
                    // Should never happen — layout returned this id.
                    error!("Pane {pane_id} not found in tree during render");
                    continue;
                };

                // Debounced resize: only send when char dims changed.
                let new_size = (pane_width_chars, pane_height_chars);
                if new_size != pane.view_state.last_sent_size {
                    if let Err(e) = pane.input_tx.send(InputEvent::Resize(
                        pane_width_chars,
                        pane_height_chars,
                        font_width,
                        font_height,
                    )) {
                        error!("Failed to send resize event for {pane_id}: {e}");
                    } else {
                        pane.view_state.last_sent_size = new_size;
                        grid_size_changed = true;
                    }
                }

                // Load this pane's snapshot and sync scroll offset.
                let pane_snap = pane.arc_swap.load();
                if pane.view_state.scroll_offset != pane_snap.scroll_offset {
                    pane.view_state.scroll_offset = pane_snap.scroll_offset;
                }

                // First-observation gate (issue #439 fix #4).
                //
                // The PTY thread publishes a new `Arc<TerminalSnapshot>` only
                // when real output arrives (a few times/sec under a settled
                // full-screen TUI like btop), but this `update()` runs every
                // frame (~60fps) and re-reads whatever snapshot is currently
                // published. `pane_snap.content_changed` is baked into the
                // snapshot at build time, so on the ~14 frames between real
                // updates it reads a stale `true`, which used to re-arm a 16ms
                // content repaint every frame — a self-perpetuating 60fps wake
                // for pixels that are not changing.
                //
                // Compare the current `visible_chars` `Arc` against the one
                // observed last frame: `is_new_snapshot` is `true` only on the
                // first frame a genuinely-new snapshot appears. We update the
                // cache unconditionally (every frame, before the widget draws)
                // — distinct from `last_rendered_visible`, which the widget
                // updates only on a full rebuild. Missing a real update is
                // impossible: every `build_snapshot` in the PTY thread is
                // paired with its own `request_repaint_after` (min-merged into
                // the wake schedule), so a new snapshot always gets at least
                // one wake independent of this gate; the gate only suppresses
                // the redundant self-scheduled repaints of already-drawn
                // content.
                let is_new_snapshot = pane
                    .render_cache
                    .observe_visible_snapshot(&pane_snap.visible_chars);

                // OSC 1338 HISTFILE reload trigger (Task 72.15).  When the
                // shell-integration scripts publish a new HISTFILE path
                // through `OSC 1338 ; HISTFILE=<path> ST`, the snapshot's
                // `shell_histfile` will diverge from the last value we
                // observed for this pane.  On change, spawn an
                // OSC-priority loader (`SEED_SEQ_OSC=1`) which CAS-wins
                // over the env-derived load published earlier at spawn
                // time.  The decision is factored into a pure function
                // (`classify_osc_reload`) so the comparison logic is
                // exhaustively unit-tested independently of egui.
                {
                    use crate::gui::shell_history::OscReloadDecision;
                    let decision = crate::gui::shell_history::classify_osc_reload(
                        pane.shell_program.as_deref(),
                        pane_snap.shell_histfile.as_deref(),
                        pane.shell_histfile_last_seen.as_deref(),
                    );
                    match decision {
                        OscReloadDecision::NoChange => {}
                        OscReloadDecision::SpawnLoad { program, path } => {
                            tracing::debug!(
                                "shell_history: pane {pane_id} OSC 1338 reload \
                                 (program={program:?}, path={path:?})"
                            );
                            crate::gui::shell_history::spawn_loader_with_path(
                                program,
                                path,
                                std::sync::Arc::clone(&pane.history_seed),
                            );
                            pane.shell_histfile_last_seen
                                .clone_from(&pane_snap.shell_histfile);
                        }
                        OscReloadDecision::NoProgramAvailable { new_path } => {
                            tracing::trace!(
                                "shell_history: pane {pane_id} OSC 1338 \
                                 received but no resolved shell program \
                                 (new_path={new_path:?}); skipping reload"
                            );
                            pane.shell_histfile_last_seen
                                .clone_from(&pane_snap.shell_histfile);
                        }
                        OscReloadDecision::Cleared => {
                            tracing::trace!(
                                "shell_history: pane {pane_id} OSC 1338 \
                                 HISTFILE cleared; leaving existing seed in place"
                            );
                            pane.shell_histfile_last_seen = None;
                        }
                    }
                }

                let is_echo_off = self.config.security.password_indicator
                    && pane.echo_off.load(std::sync::atomic::Ordering::Relaxed);
                let is_active = pane_id == active_pane_id;

                // Broadcast input (Task 74): only the active pane fans out its
                // keyboard input, and only to the *other* panes. Non-active
                // panes and the broadcast-off case pass an empty slice.
                let key_broadcast_targets: Vec<crossbeam_channel::Sender<InputEvent>> = if is_active
                {
                    broadcast_senders
                        .iter()
                        .filter(|(id, _)| *id != pane_id)
                        .map(|(_, tx)| tx.clone())
                        .collect()
                } else {
                    Vec::new()
                };

                // Build a RecordingContext for this pane if recording is active.
                // Hold the Arc locally so the borrow in `RecordingContext.handle`
                // remains valid for the lifetime of `rec_ctx`.
                let rec_window_id = self.recording_window_id(window_id);
                let rec_handle = self.recording_swap.load_full();
                let rec_ctx = rec_handle.as_ref().map(|h| {
                    freminal_terminal_emulator::recording::RecordingContext {
                        handle: h,
                        window_id: rec_window_id,
                        // Saturating `u64 -> u32` for recording: pane IDs are
                        // monotonic from 0 and never approach u32::MAX.
                        pane_id: u32::try_from(pane_id.raw()).unwrap_or(u32::MAX),
                    }
                });

                // Render this pane into a child UI scoped to its content rect.
                // show() returns (left_clicked, copied_to_clipboard, deferred_key_actions).
                // left_clicked is true when a primary left-click was pressed inside
                // this pane's rect — used below for click-to-focus. copied_to_clipboard
                // is true when a non-empty local selection was copied this frame
                // (Subtask D3), used below to route a "Copied to clipboard" toast.
                // Task 121 frame-profiling harness: time only this pane's
                // `terminal_widget.show()` call (via the `scope_builder`
                // wrapper), summed across every pane into
                // `phase_panes_this_frame`.
                #[cfg(feature = "frame-profiling")]
                let pane_show_start = std::time::Instant::now();
                let show_result =
                    ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |pane_ui| {
                        win.terminal_widget.show(
                            pane_ui,
                            &pane_snap,
                            &mut pane.view_state,
                            &pane.render_state,
                            &mut pane.render_cache,
                            &pane.input_tx,
                            &pane.clipboard_rx,
                            &pane.search_buffer_rx,
                            ui_overlay_open,
                            border_drag_active,
                            bg_opacity,
                            self.config.ui.background_image_opacity,
                            self.config.ui.background_image_mode,
                            &self.config.command_blocks,
                            gutter_inset_logical,
                            &self.binding_map,
                            is_echo_off,
                            is_active,
                            pane_id,
                            rec_ctx.as_ref(),
                            &mut pane.pending_copy,
                            &key_broadcast_targets,
                            &present_is_partial_for_panes,
                            split_border_hover,
                        )
                    });
                #[cfg(feature = "frame-profiling")]
                {
                    phase_panes_this_frame += pane_show_start.elapsed();
                }
                let (left_clicked, copied_to_clipboard, deferred_actions) = show_result.inner;
                all_deferred_actions.extend(deferred_actions);

                // Subtask 121.12: drain this pane's in-frame repaint needs
                // (bell flash, cursor trail, animated image, gutter hover,
                // scrollbar damage — folded into `pane.render_cache` by
                // `show()` rather than requested on the `Context` directly)
                // and fold the shortest into the frame-wide aggregate so the
                // need is visible to `App::take_terminal_requested_delay`.
                if let Some(delay) = pane.render_cache.take_pending_repaint_delay() {
                    shortest_repaint_delay =
                        Some(shortest_repaint_delay.map_or(delay, |prev| prev.min(delay)));
                }

                // Subtask 122.15: lift this pane's terminal-rect origin —
                // computed by `show()` above and recorded into
                // `pane.render_cache.terminal_rect_origin` — into the
                // published, out-of-frame-readable type. Read directly from
                // the cache (not recomputed from `content_rect` +
                // `gutter_inset_logical`) so the published value can never
                // drift from what `show` actually drew.
                win.published
                    .publish_pane_terminal_origin(pane_id, pane.render_cache.terminal_rect_origin);

                if copied_to_clipboard {
                    self.route_freminal_toast(
                        freminal_common::config::FreminalToastCategory::ClipboardCopy,
                        crate::gui::toast::ToastKind::Info,
                        "Copied to clipboard",
                        None,
                        crate::gui::toast::ToastPlacement::pane_centered(window_id, pane_id),
                    );
                }

                // Task 114.7: drain any egui-blocked raw key events queued
                // this frame (keypad operators/directional, media,
                // print/pause/menu keys) for the active pane. Must run
                // here, after `show()` returned above, so
                // `pane.render_cache.super_pressed()` reflects the current
                // frame — draining earlier (or inside `on_raw_key_event`
                // itself) risks encoding against a stale Super state.
                if is_active && !win.pending_raw_keys.is_empty() {
                    // Per-pane overlays (search, command-history palette,
                    // right-click context menu) also own keyboard input and
                    // suppress normal terminal input; queued raw keys must be
                    // gated the same way so they cannot bypass those overlays.
                    // A pane-border drag-to-resize is another suppression
                    // cause (#456 review): the same "no terminal input while
                    // a border drag is active" invariant that `show()` is
                    // handed via `border_drag_active` must also apply here,
                    // or queued raw keys leak keypad/media keys to the PTY
                    // mid-resize.
                    let pane_input_suppressed = pane.view_state.search_state.is_open
                        || pane.view_state.command_history.is_open
                        || pane.view_state.context_menu_pos.is_some();
                    if ui_overlay_open || pane_input_suppressed || border_drag_active {
                        // An overlay (rename/paste/close/broadcast dialog,
                        // menu, welcome/about window, save-layout, or a
                        // per-pane search/history/context menu) owns keyboard
                        // input this frame — the same gate that suppresses
                        // normal terminal input. A pane-border drag-to-resize
                        // in progress is the same case: the divider owns
                        // input this frame. Drop the queued raw keys instead
                        // of forwarding them to the PTY, so intercepted keys
                        // cannot bypass the overlay or leak during a resize.
                        win.pending_raw_keys.clear();
                    } else {
                        let super_pressed = pane.render_cache.super_pressed();
                        crate::gui::terminal::input::drain_pending_raw_keys(
                            &mut win.pending_raw_keys,
                            &pane.input_tx,
                            &pane_snap,
                            super_pressed,
                            &key_broadcast_targets,
                        );
                    }
                }

                // ── Command history palette overlay (Ctrl+Shift+M) ───
                // Rendered here (not in `widget.show`) because the palette
                // needs `Pane`-owned data — `recent_commands`,
                // `history_seed`, and the `command_texts` cache — that the
                // widget does not have access to.  The palette is an
                // `egui::Area` overlay so its render order relative to the
                // widget body does not matter; what matters is that
                // `Pane` is in scope here.
                if pane.view_state.command_history.is_open {
                    use crate::gui::command_history::PaletteAction;
                    // Hold the Arc for the duration of the palette call
                    // so the borrow into `entries` remains valid.
                    let seed_arc = pane.history_seed.load_full();
                    let seed: Option<&Vec<String>> = if seed_arc.entries.is_empty() {
                        None
                    } else {
                        Some(seed_arc.entries.as_ref())
                    };
                    let action = crate::gui::command_history::show_command_history_palette(
                        ui,
                        &mut pane.view_state.command_history,
                        content_rect,
                        pane_id,
                        seed,
                        &pane.recent_commands,
                        &pane.command_texts,
                    );
                    match action {
                        PaletteAction::None => {}
                        PaletteAction::Close => {
                            pane.view_state.command_history.close();
                            crate::gui::command_history::log_close(pane_id);
                        }
                        PaletteAction::Submit(text) => {
                            let len = text.len();
                            let ok = crate::gui::command_history::send_command_text(
                                &pane.input_tx,
                                &text,
                            );
                            if !ok {
                                crate::gui::command_history::log_submit_failure(pane_id, len);
                            }
                            pane.view_state.command_history.close();
                            crate::gui::command_history::log_close(pane_id);
                        }
                    }
                }

                // Focus transfer (Task 110): a non-active pane is focused either
                // by an explicit left-click or (when focus-follows-mouse is
                // enabled) by the mouse hovering it. Following the mouse only
                // changes the *focused* (keyboard target) pane; it does not
                // retarget in-flight mouse input. Tab switching is unaffected.
                // The pointer is over at most one pane at a time, so this cannot
                // flicker between panes within a frame.
                let pointer_over_content = ui
                    .ctx()
                    .pointer_hover_pos()
                    .is_some_and(|pos| content_rect.contains(pos));
                // Freeze focus while dragging a pane divider so the active pane
                // doesn't flicker between panes as the pointer moves (issue
                // #453). On release, focus is set to the pane under the
                // pointer (see the drag_stopped handler).
                let should_focus = !is_active
                    && !border_drag_active
                    && crate::gui::panes::should_focus_inactive_pane(
                        left_clicked,
                        self.config.tabs.focus_follows_mouse,
                        pointer_over_content,
                    );
                if should_focus {
                    let tab = win.tabs.active_tab_mut();
                    let old_active = tab.active_pane;
                    // Notify the previously-active pane that it lost focus.
                    if let Some(old_pane) = tab.pane_tree.find(old_active)
                        && let Err(e) = old_pane.input_tx.send(InputEvent::FocusChange(false))
                    {
                        error!("Failed to send FocusChange(false) to pane {old_active}: {e}");
                    }
                    // Switch focus.
                    tab.active_pane = pane_id;
                    // Notify the newly-active pane that it gained focus.
                    if let Some(new_pane) = tab.pane_tree.find(pane_id)
                        && let Err(e) = new_pane.input_tx.send(InputEvent::FocusChange(true))
                    {
                        error!("Failed to send FocusChange(true) to pane {pane_id}: {e}");
                    }
                }

                // Advance text blink cycle for this pane if it has blinking text.
                if pane_snap.has_blinking_text {
                    // Re-borrow after the allocate_new_ui closure.
                    let tab = win.tabs.active_tab_mut();
                    if let Some(p) = tab.pane_tree.find_mut(pane_id) {
                        p.view_state.tick_text_blink();
                    }
                }

                // Determine repaint delay for this pane.
                //
                // A blink-style cursor only needs the periodic ~500ms wake
                // when the cursor is ACTUALLY on screen — i.e. exactly the
                // condition the drawing side gates on (`effective_show_cursor`
                // in `terminal/widget.rs`: `snap.show_cursor && !is_echo_off
                // && is_active_pane`). Scheduling the wake off the configured
                // cursor *style* alone (ignoring `show_cursor`) is a real
                // over-repaint bug: a full-screen TUI that hides the cursor
                // via DECTCEM (`\e[?25l`) — btop, vim, htop, less, … — keeps
                // the default blink *style*, so the terminal wakes ~2x/sec to
                // redraw an unchanged, cursor-hidden screen forever. Gating on
                // the real cursor visibility lets those idle-at-hidden-cursor
                // frames drop to zero.
                let cursor_blink_wants_repaint = cursor_blink_wants_repaint(
                    &pane_snap.cursor_visual_style,
                    pane_snap.show_cursor,
                    is_active,
                    is_echo_off,
                );
                // Honour `content_changed` only on the first observation of a
                // genuinely-new snapshot (issue #439 fix #4). Re-reading the
                // same published `Arc` on a later frame sees the same
                // (byte-identical) pixels, so scheduling another content
                // repaint buys nothing and only perpetuates the 60fps wake.
                let content_wants_repaint = is_new_snapshot && pane_snap.content_changed;
                // Diagnostic: count the ~2Hz phantom wakes the `show_cursor`
                // gate now suppresses — a blink-STYLE cursor on the active
                // pane, content unchanged, no blinking text, but the cursor
                // is actually hidden (DECTCEM / echo-off), so the old code
                // would have scheduled a 500ms wake and the new code does not.
                // Uses `content_wants_repaint` (the gated signal actually
                // driving scheduling) rather than the raw sticky flag so the
                // counter tracks the real scheduling decision.
                if is_active
                    && !content_wants_repaint
                    && !pane_snap.has_blinking_text
                    && !cursor_blink_wants_repaint
                    && matches!(
                        pane_snap.cursor_visual_style,
                        freminal_common::cursor::CursorVisualStyle::BlockCursorBlink
                            | freminal_common::cursor::CursorVisualStyle::UnderlineCursorBlink
                            | freminal_common::cursor::CursorVisualStyle::VerticalLineCursorBlink,
                    )
                {
                    win.frame_stats.blink_wake_suppressed =
                        win.frame_stats.blink_wake_suppressed.saturating_add(1);
                }
                if content_wants_repaint
                    || cursor_blink_wants_repaint
                    || pane_snap.has_blinking_text
                {
                    let delay = if content_wants_repaint {
                        std::time::Duration::from_millis(16)
                    } else if pane_snap.has_blinking_text {
                        view_state::TEXT_BLINK_TICK_DURATION
                    } else {
                        std::time::Duration::from_millis(500)
                    };
                    shortest_repaint_delay =
                        Some(shortest_repaint_delay.map_or(delay, |prev| prev.min(delay)));
                }
            }

            // Resize overlay (issue #433): show a transient cols×rows HUD only
            // on a genuine OS-window resize, not the last_sent_size churn from
            // new tabs/splits/pane-close/zoom (which reset last_sent_size to
            // (0, 0)) nor the first geometry observation on window creation.
            // Split-border drags are excluded: the whole-window readout does
            // not change when an internal border moves. See `resize_is_genuine`
            // and `window_genuinely_resized`.
            //
            // `grid_size_changed` is only a TRIGGER (did any pane's char
            // grid change this frame); the displayed dimensions are the
            // WHOLE terminal content area, not one pane's — the overlay is
            // window-scoped. Compute cols×rows from `available_rect` (the
            // central content rect the overlay centers in) using the same
            // cell size and gutter inset the per-pane sizing uses. For a
            // single-pane window this is exact; for a split layout it is an
            // approximate whole-window reading (each pane pays the gutter
            // individually), which is acceptable for a transient readout.
            if self.config.notifications.show_resize_overlay
                && super::window::resize_is_genuine(grid_size_changed, window_genuinely_resized)
            {
                let window_content_width = (available_rect.width() - gutter_inset_logical).max(0.0);
                let window_cols = (window_content_width / logical_char_w)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or(0)
                    .max(1);
                let window_rows = (available_rect.height() / logical_char_h)
                    .floor()
                    .approx_as::<usize>()
                    .unwrap_or(0)
                    .max(1);
                win.published
                    .start_resize_overlay(super::window::ResizeOverlayState {
                        size: (window_cols, window_rows),
                        last_update: now,
                    });
            }

            // ── Frame-damage aggregation (#435) ───────────────────────
            //
            // Decide whether this whole frame was a pure cursor-only update,
            // so the windowing layer may skip the full-framebuffer clear and
            // present only the changed cursor region. This is deliberately
            // conservative: `Partial` is emitted ONLY when every condition
            // below positively proves nothing but the cursor blinked/moved.
            // Any doubt falls through to `Full` (a normal clear + present),
            // which is always correct. The windowing layer additionally
            // requires `buffer_age() == 1` before honoring `Partial`, so a
            // false positive here still cannot corrupt the frame on a fresh
            // or aged back buffer — but we avoid false positives regardless.
            //
            // See `stage_frame_damage`'s doc for the full decision rationale,
            // the mandatory double-write contract with
            // `compose_with_chrome_damage` (write #2, near the end of
            // `update()`), and why `pointer_over_chrome` must hit-test `win`'s
            // own chrome rects rather than call `self.is_chrome_interactive_at`.
            let (frame_damage, damage_obs) = stage_frame_damage(
                &win,
                ctx,
                &pane_layout,
                active_pane_id,
                &FrameDamageInputs {
                    ui_overlay_open,
                    active_pane_changed,
                    border_drag_active,
                },
                &self.toasts,
            );
            win.pending_frame_damage = frame_damage;
            let shader_recomposites = damage_obs.shader_recomposites;
            let toast_active = damage_obs.toast_active;
            let foreground_overlay_open = damage_obs.foreground_overlay_open;
            let per_pane_damage = damage_obs.per_pane_damage;
            let active_pane_damage = damage_obs.active_pane_damage;

            // Task 121 frame-profiling harness (feature-gated): chrome-mode
            // duty cycle and zero-pixel-change-but-presented counters.
            //
            // `chrome_mode` answers "does `ChromeMode::Replay` ever actually
            // engage in a live session, and at what duty cycle" -- no
            // counter for this existed anywhere before this harness.
            //
            // `zero_change_presented` counts frames where every pane in
            // `per_pane_damage` reported `Unchanged` (no bell) and neither
            // `force_full`/`unresolved_pane` nor `toast_active` applied --
            // exactly the case `decide_frame_damage` has no representation
            // for "nothing changed" distinct from "something needs a full
            // rebuild": both fall through to `FrameDamage::Full` once the
            // collected damage-rect list is empty (see that function's
            // step 4). This is measurement only -- the fallback-to-Full
            // behavior is NOT changed here.
            //
            // Subtask 122.9: only the RECORDING stays feature-gated here --
            // `force_full`/`unresolved_pane`/`toast_active`/`per_pane_damage`
            // were computed unconditionally by `stage_frame_damage`, above,
            // per the Subtask 122.5 contract (see that function's doc).
            #[cfg(feature = "frame-profiling")]
            {
                let stats = &mut win.frame_stats;
                match chrome_mode {
                    freminal_windowing::ChromeMode::Full => {
                        stats.chrome_mode_full = stats.chrome_mode_full.saturating_add(1);
                    }
                    freminal_windowing::ChromeMode::Replay => {
                        stats.chrome_mode_replay = stats.chrome_mode_replay.saturating_add(1);
                    }
                }
                let all_panes_unchanged = !damage_obs.force_full
                    && !damage_obs.unresolved_pane
                    && !toast_active
                    && per_pane_damage.iter().all(|p| {
                        !p.bell_active
                            && matches!(
                                p.cursor_damage,
                                crate::gui::renderer::PaneFrameDamage::Unchanged
                            )
                    });
                if all_panes_unchanged {
                    stats.zero_change_presented = stats.zero_change_presented.saturating_add(1);
                }
                // `frame_damage_full`/`frame_damage_partial` are NOT counted
                // here (Task 121 defect-2 fix): `win.pending_frame_damage` at
                // this point is still the PRE-composition value --
                // `compose_with_chrome_damage` (near the end of `update()`,
                // after this closure returns) can upgrade a `Partial` here to
                // `Full`, which would otherwise undercount `Full` and
                // overcount `Partial`. Counted instead from the final,
                // post-composition value at the recording point after that
                // composition -- see the block near `compose_with_chrome_damage`.
            }

            // Diagnostic frame-attribution stats (reuses the #435 damage
            // signal). One `update()` == one drawn frame; classify the active
            // pane's damage so a CPU investigation can see, without a
            // profiler, how many drawn frames were genuinely-unchanged
            // (no CPU rebuild), cursor-only patches, or full vertex rebuilds.
            {
                let stats = &mut win.frame_stats;
                stats.frames_drawn = stats.frames_drawn.saturating_add(1);
                match active_pane_damage {
                    Some(crate::gui::renderer::PaneFrameDamage::Unchanged) => {
                        stats.unchanged = stats.unchanged.saturating_add(1);
                    }
                    Some(crate::gui::renderer::PaneFrameDamage::CursorOnly(_)) => {
                        stats.cursor_only = stats.cursor_only.saturating_add(1);
                    }
                    Some(crate::gui::renderer::PaneFrameDamage::Full) | None => {
                        stats.full = stats.full.saturating_add(1);
                    }
                }
                if stats
                    .frames_drawn
                    .is_multiple_of(super::window::FrameStats::FLUSH_EVERY)
                {
                    // `debug!`, not `info!`: this is an investigative
                    // diagnostic for chasing idle-CPU / over-repaint issues,
                    // not end-user-facing telemetry. At `info` it would write
                    // a line to the user's persistent log every ~2s of active
                    // use forever; keep it out of the default log stream.
                    tracing::debug!(
                        frames_drawn = stats.frames_drawn,
                        unchanged = stats.unchanged,
                        cursor_only = stats.cursor_only,
                        full = stats.full,
                        blink_wake_suppressed = stats.blink_wake_suppressed,
                        "frame-attribution stats (#439 diag): drawn frames split \
                         by active-pane damage class; blink_wake_suppressed = \
                         ~2Hz phantom wakes avoided by the show_cursor gate"
                    );
                }
            }

            // ── Chrome-damage signals (#436.3 §3.3) ───────────────────
            //
            // Stage the individual §3.3 signals this frame, computed from
            // values already available here (several — `ui_overlay_open`,
            // `active_pane_changed`, `shader_recomposites`, `ppp_changed`,
            // `chrome_focus_changed`, per-pane bell state — only exist inside
            // this `CentralPanel` closure). The final `ChromeDamage` decision
            // additionally needs the after-toast-render dismissible-presence
            // sample, which can only be taken once this closure returns (the
            // toast overlay renders after it) — so `win.published`'s staged
            // chrome signals are a staging value, combined into
            // `win.pending_chrome_damage` near the end of `update()`.
            let (chrome_tab_snapshot, chrome_signals) = stage_chrome_signals(
                &win,
                self.config.tab_title.policy,
                &self.config.tab_title.separator,
                &pane_layout,
                zoomed_pane,
                &per_pane_damage,
                &ChromeSignalInputs {
                    ui_overlay_open,
                    chrome_style_changed,
                    active_pane_changed,
                    shader_recomposites,
                    toast_active,
                    chrome_size_changed,
                    ppp_changed,
                    chrome_focus_changed,
                    chrome_warming_up,
                    foreground_overlay_open,
                },
            );
            win.prev_chrome_tab_snapshot = chrome_tab_snapshot;
            win.published.publish_pending_chrome_signals(chrome_signals);

            // Task 121 frame-profiling harness follow-up (issue #459/#461
            // gate-blocker investigation): count which individual §3.3
            // signal(s) fired this frame, indexed identically to
            // `ChromeSignals::named_fields()`'s exhaustive destructure (see
            // that method's doc for why a future 16th field cannot be
            // silently missed here). More than one signal can fire the same
            // frame; every one that did gets counted, not just the first.
            #[cfg(feature = "frame-profiling")]
            for (i, (_, fired)) in win
                .published
                .pending_chrome_signals()
                .named_fields()
                .into_iter()
                .enumerate()
            {
                if fired {
                    win.frame_stats.chrome_signal_fired_counts[i] =
                        win.frame_stats.chrome_signal_fired_counts[i].saturating_add(1);
                }
            }

            // ── Window-level post-processing pass ────────────────────
            //
            // When a user GLSL shader is active, the window FBO now contains
            // the composited terminal content from all panes.  We draw it
            // through the user shader back to egui's framebuffer.
            //
            // This callback is registered BEFORE pane borders so the borders
            // are painted on top of the shader output.
            {
                let wpr_check = win
                    .window_post
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                let shader_active = wpr_check.is_active();
                let pending = wpr_check.pending_shader.is_some();
                drop(wpr_check);
                if shader_active || pending {
                    let frame_dt = ui.input(|i| i.stable_dt);
                    let wpr_for_post = Arc::clone(&win.window_post);
                    ui.painter().add(egui::PaintCallback {
                        rect: available_rect,
                        callback: Arc::new(CallbackFn::new(move |info, painter| {
                            let gl = &Gl::real(painter.gl());
                            let vp = info.viewport_in_pixels();
                            let mut wpr = wpr_for_post
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);

                            // Lazy-init GPU resources.
                            if !wpr.initialized()
                                && let Err(e) = wpr.init(gl)
                            {
                                error!("WindowPostRenderer init failed: {e}");
                                wpr.last_error = Some(format!("Renderer init failed: {e}"));
                                return;
                            }

                            // Process any pending shader change.
                            if let Some(pending_shader) = wpr.pending_shader.take() {
                                match pending_shader {
                                    Some(src)
                                        if let Err(e) = wpr.update_shader(
                                            gl,
                                            &src,
                                            vp.width_px,
                                            vp.height_px,
                                        ) =>
                                    {
                                        error!("Shader compilation failed: {e}");
                                        wpr.last_error =
                                            Some(format!("Shader compile failed: {e}"));
                                    }
                                    Some(_) => {}
                                    None => wpr.clear_shader(gl),
                                }
                            }

                            // Apply the post-processing pass if the shader is active.
                            if wpr.is_active() {
                                wpr.ensure_fbo(gl, vp.width_px, vp.height_px);
                                // Bind egui's framebuffer as the render target.
                                unsafe {
                                    gl.bind_framebuffer(
                                        glow::FRAMEBUFFER,
                                        painter.intermediate_fbo(),
                                    );
                                }

                                let vp_w = vp.width_px.approx_as::<f32>().unwrap_or(0.0);
                                let vp_h = vp.height_px.approx_as::<f32>().unwrap_or(0.0);
                                wpr.draw_post_pass(gl, vp_w, vp_h, frame_dt);
                            }
                        })),
                    });

                    // When the shader is active, request continuous repaints so
                    // the `u_time` uniform advances smoothly (~60 fps).
                    if shader_active {
                        let anim_delay = std::time::Duration::from_millis(16);
                        shortest_repaint_delay = Some(
                            shortest_repaint_delay.map_or(anim_delay, |prev| prev.min(anim_delay)),
                        );
                    }
                }
            }

            // ── Pane borders ─────────────────────────────────────────
            //
            // Draw "surround the active pane" highlighted borders (Task 109).
            // Every edge of the active pane that is an interior divider is
            // highlighted full-length in the active color; the rest of each
            // divider is inactive. Outer window edges are never dividers, so
            // they are never highlighted. Each pane's own edges light up, so
            // stacked / nested panes stay distinguishable (a middle stacked
            // pane lights its top AND bottom; its neighbours light only the
            // shared edge).
            //
            // The one exception is a tab with EXACTLY two panes: they share a
            // single full-span divider, so surrounding either pane lights the
            // same line and the focused pane is indistinguishable. In that
            // case the divider is half-filled on the active pane's side
            // (the classic tmux behaviour).
            let broadcast_active = win.tabs.active_tab().broadcast_input;
            if has_multiple_panes && zoomed_pane.is_none() {
                let painter = ui.painter();
                // Broadcast mode (Task 74) tints every split border yellow so
                // the user has a constant visual reminder that keystrokes are
                // fanning out to every pane.  Otherwise the active pane's
                // edges use the theme's bright-blue (ansi[12]) — the themed
                // equivalent of the original hardcoded blue, distinct from the
                // command-block status-gutter colors (green/red/yellow) — and
                // the rest are gray.
                let (inactive_color, active_color) = if broadcast_active {
                    (
                        egui::Color32::from_rgb(180, 150, 40),
                        egui::Color32::from_rgb(240, 200, 60),
                    )
                } else {
                    let theme = freminal_common::themes::by_slug(
                        self.config.theme.active_slug(win.os_dark_mode),
                    )
                    .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
                    let (br, bg, bb) = theme.ansi[12];
                    (
                        egui::Color32::from_gray(80),
                        egui::Color32::from_rgb(br, bg, bb),
                    )
                };

                // Rect of the currently focused pane; used to decide which
                // divider segments border it. Converted once here (rather
                // than per-border below) since `active_highlight_segment`
                // takes the toolkit-neutral `geometry::Rect`.
                let active_rect = pane_layout
                    .iter()
                    .find(|(id, _)| *id == active_pane_id)
                    .map(|(_, r)| geometry_interop::rect_from_egui(*r));

                let border_rects = win
                    .tabs
                    .active_tab()
                    .pane_tree
                    .split_borders(
                        geometry_interop::rect_from_egui(available_rect),
                        active_pane_id,
                    )
                    .unwrap_or_default();

                // Tolerance for matching a divider coordinate to a pane edge.
                let edge_epsilon: f32 = 1.0;

                // Exactly-two-pane tabs share a single divider; half-fill it
                // on the active pane's side rather than surrounding (which
                // would be ambiguous). `pane_layout` holds every leaf rect.
                let exactly_two_panes = pane_layout.len() == 2;

                let stroke = |from, to, color| {
                    painter.line_segment([from, to], egui::Stroke::new(border_width, color));
                };

                for border in &border_rects {
                    // `SplitBorder::rect` is the toolkit-neutral
                    // `geometry::Rect`; this loop paints with egui-only
                    // methods (`left_top`/`left_bottom`/`right_top`), so
                    // convert per-border here at the point of use.
                    let r = geometry_interop::rect_to_egui(border.rect);

                    if exactly_two_panes {
                        // Half-fill: the active pane's side gets the active
                        // color; the other half stays inactive.
                        // `active_subtree` is `First` when the active pane is
                        // the first child (top for a vertical line, left for
                        // a horizontal line).
                        let (first_color, second_color) = match border.active_subtree {
                            panes::ActiveSubtree::First => (active_color, inactive_color),
                            panes::ActiveSubtree::Second => (inactive_color, active_color),
                            panes::ActiveSubtree::Neither => (inactive_color, inactive_color),
                        };
                        match border.direction {
                            panes::SplitDirection::Horizontal => {
                                // Vertical line — split top/bottom.
                                let mid_y = f32::midpoint(r.min.y, r.max.y);
                                stroke(r.left_top(), egui::pos2(r.min.x, mid_y), first_color);
                                stroke(egui::pos2(r.min.x, mid_y), r.left_bottom(), second_color);
                            }
                            panes::SplitDirection::Vertical => {
                                // Horizontal line — split left/right.
                                let mid_x = f32::midpoint(r.min.x, r.max.x);
                                stroke(r.left_top(), egui::pos2(mid_x, r.min.y), first_color);
                                stroke(egui::pos2(mid_x, r.min.y), r.right_top(), second_color);
                            }
                        }
                        continue;
                    }

                    // 3+ panes: surround. The whole divider is drawn inactive
                    // first…
                    match border.direction {
                        panes::SplitDirection::Horizontal => {
                            stroke(r.left_top(), r.left_bottom(), inactive_color);
                        }
                        panes::SplitDirection::Vertical => {
                            stroke(r.left_top(), r.right_top(), inactive_color);
                        }
                    }

                    // …then the segment along the active pane's edge is
                    // redrawn full-length in the active color.
                    if let Some(seg) = active_rect
                        .and_then(|ar| panes::active_highlight_segment(border, ar, edge_epsilon))
                    {
                        let seg = geometry_interop::rect_to_egui(seg);
                        match border.direction {
                            panes::SplitDirection::Horizontal => {
                                stroke(seg.left_top(), seg.left_bottom(), active_color);
                            }
                            panes::SplitDirection::Vertical => {
                                stroke(seg.left_top(), seg.right_top(), active_color);
                            }
                        }
                    }
                }
            }

            // Broadcast label (Task 74): when broadcast is active, paint a
            // small "BROADCAST" tag in the top-right corner of every visible
            // pane.  Top-right is chosen so it never collides with the
            // password-prompt lock icon (which lives in the tab/menu bar, not
            // the pane area).  Drawn for the zoomed pane too.
            if broadcast_active {
                let painter = ui.painter();
                let label_color = egui::Color32::from_rgb(240, 200, 60);
                let bg = egui::Color32::from_rgba_unmultiplied(0, 0, 0, 160);
                for (_pane_id, pane_rect) in &pane_layout {
                    let anchor = egui::pos2(pane_rect.max.x - 4.0, pane_rect.min.y + 4.0);
                    let galley = painter.layout_no_wrap(
                        "BROADCAST".to_owned(),
                        egui::FontId::monospace(10.0),
                        label_color,
                    );
                    let text_rect = egui::Align2::RIGHT_TOP
                        .anchor_size(anchor, galley.size())
                        .expand(2.0);
                    painter.rect_filled(text_rect, 2.0, bg);
                    painter.galley(
                        text_rect.left_top() + egui::vec2(2.0, 2.0),
                        galley,
                        label_color,
                    );
                }
            }

            // Resize overlay HUD (issue #433): a passive, window-centered
            // "cols × rows" readout, drawn on the plain painter like the
            // broadcast label above — no input, no `ui_overlay_open`
            // registration needed. Bind the `Copy` state up front so the
            // subsequent clear-on-timeout (below) doesn't conflict with the
            // `ui.painter()` borrow used to draw it.
            let resize_overlay_state = win.published.resize_overlay();
            if let Some(overlay) = resize_overlay_state {
                let elapsed = now.saturating_duration_since(overlay.last_update);
                if elapsed >= super::window::RESIZE_OVERLAY_LINGER {
                    win.published.clear_resize_overlay();
                } else {
                    let alpha = super::window::resize_overlay_alpha(
                        elapsed,
                        super::window::RESIZE_OVERLAY_LINGER,
                        super::window::RESIZE_OVERLAY_FADE,
                    );
                    let text_alpha = (255.0 * alpha)
                        .round()
                        .clamp(0.0, 255.0)
                        .approx_as::<u8>()
                        .unwrap_or(255);
                    let bg_alpha = (180.0 * alpha)
                        .round()
                        .clamp(0.0, 255.0)
                        .approx_as::<u8>()
                        .unwrap_or(180);
                    let (cols, rows) = overlay.size;
                    let text_color =
                        egui::Color32::from_rgba_unmultiplied(255, 255, 255, text_alpha);
                    let painter = ui.painter();
                    let galley = painter.layout_no_wrap(
                        format!("{cols} × {rows}"),
                        egui::FontId::monospace(28.0),
                        text_color,
                    );
                    let text_rect = egui::Align2::CENTER_CENTER
                        .anchor_size(available_rect.center(), galley.size())
                        .expand(12.0);
                    painter.rect_filled(text_rect, 6.0, egui::Color32::from_black_alpha(bg_alpha));
                    painter.galley(text_rect.min + egui::vec2(12.0, 12.0), galley, text_color);
                    // Keep animating/timing-out even without further input.
                    // Subtask 121.12: fold into `shortest_repaint_delay`
                    // (same closure, in scope here) instead of calling
                    // `ctx.request_repaint_after()` directly — a direct call
                    // would be invisible to `effective_repaint_delay`'s
                    // suppressed-pointer substitution and would be silently
                    // downgraded to the fallback interval while the mouse is
                    // moving over terminal content. Only the delivery
                    // mechanism changes; the overlay's own timing/alpha is
                    // untouched.
                    //
                    // Subtask 121.14: request only the delay actually
                    // needed rather than an unconditional 16ms every frame
                    // the HUD is alive — a wake timed to land exactly at
                    // fade-start while still opaque, then 16ms once
                    // genuinely animating. This MUST stay consistent with
                    // `resize_overlay_is_animating` (the suppression
                    // predicate `pointer_motion_needs_repaint` uses): if the
                    // two disagree, the HUD either janks (suppression
                    // engages while this still requested a fast wake) or
                    // never sleeps (this keeps requesting 16ms while
                    // suppression was already judged safe). Without this
                    // step, Part A's suppression fix would be worthless: the
                    // unconditional 16ms request folded into
                    // `shortest_repaint_delay` every frame would still
                    // schedule the window at 60fps regardless of what the
                    // suppression predicate allowed.
                    let hud_delay = super::window::resize_overlay_repaint_delay(
                        elapsed,
                        super::window::RESIZE_OVERLAY_LINGER,
                        super::window::RESIZE_OVERLAY_FADE,
                    );
                    shortest_repaint_delay =
                        Some(shortest_repaint_delay.map_or(hud_delay, |prev| prev.min(hud_delay)));
                }
            }

            // ── Terminal band: capture the end of the shape range (#436.4a) ──
            //
            // Read the background layer's current shape count as
            // `band_shape_end`, so `[band_shape_start, band_shape_end)` is
            // exactly the band's range within `LayerId::background()`'s
            // `PaintList`. `run_frame` slices `full_output.shapes` by this
            // range directly (the background layer drains first into
            // `full_output.shapes`), so — unlike 436.2a's approach — nothing
            // is cloned here: this is a pure read of the shape count, and
            // the real shapes are left in place in
            // `LayerId::background()`'s `PaintList` and drain into
            // `FullOutput.shapes` exactly as before. `run_frame`'s 3-way
            // split (head/band/tail, each tessellated and painted
            // separately, in that order) reconstructs the same total shape
            // set with the same paint order as the pre-#436.4a single-call
            // path, so this frame renders byte-identically to before.
            //
            // Capture point justification (end): nothing between the
            // broadcast-label loop above and here paints into the background
            // layer either — this is the very next statement — so
            // `band_shape_end` is exactly the count after the pre-clear
            // callback through the broadcast label, i.e. exactly the end of
            // the band.
            let band_shape_end = ctx.graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(band_shape_start, |list| list.all_entries().len())
            });
            win.pending_terminal_band_range = band_shape_start..band_shape_end;

            // Handle key actions that couldn't be dispatched at the input
            // layer because they require full GUI state.
            for action in all_deferred_actions {
                self.dispatch_deferred_action(action, &mut win, window_id, handle);
            }

            // Drain a pending paste injected by the windowing layer's
            // `Event::Paste` (Task 77). Use the already-read text; do not
            // re-read the clipboard here. `bypass_guard` (the PasteUnsafe
            // action, Ctrl+Shift+Alt+V) sends directly without analysis.
            let pending_paste = win
                .tabs
                .active_tab_mut()
                .active_pane_mut()
                .and_then(|pane| pane.view_state.pending_paste.take());
            if let Some(pending) = pending_paste {
                if pending.bypass_guard {
                    Self::send_paste_to_active_pane(&mut win, pending.text);
                } else {
                    self.guarded_paste_text(&mut win, pending.text);
                }
            }

            // Handle deferred close-pane (needs `ui` for ViewportCommand::Close).
            // Routed through the close guard (Task 98): a running foreground
            // command opens the confirmation dialog instead of closing.
            if win.pending_close_pane {
                win.pending_close_pane = false;
                self.guarded_close_pane(ui, &mut win);
            }

            // Handle deferred directional focus (needs layout rects).
            if let Some(dir) = win.pending_focus_direction.take() {
                Self::focus_pane_in_direction(dir, available_rect, &mut win);
            }

            // Keep the window title bar in sync with the active tab's title.
            // This handles tab switches, OSC 0/2 title changes, and restore
            // from the title stack — all in one place.
            //
            // The window title is resolved under the configured tab-title
            // policy (`[tab_title] policy`), combining the user-assigned
            // custom name with the shell-asserted OSC title.  Under the
            // `OscWins` policy a shell title clears the custom name; under
            // every other policy the custom name persists.
            //
            // Only issue the viewport command when the title actually changed;
            // calling `send_viewport_cmd` unconditionally every frame triggers
            // an infinite repaint loop (~3 % idle CPU).
            let active_tab = win.tabs.active_tab();
            let active_title = active_tab.display_name(
                self.config.tab_title.policy,
                &self.config.tab_title.separator,
            );
            let window_title = if active_title.is_empty() {
                "Freminal"
            } else {
                active_title.as_ref()
            };
            if window_title != win.last_window_title {
                window_title.clone_into(&mut win.last_window_title);
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(win.last_window_title.clone()));
            }

            // Stash this frame's own repaint request for #436.4b's
            // `chrome_repaint_settled` gate (drained by
            // `App::take_terminal_requested_delay`): the NEXT frame's replay
            // decision needs to know what delay THIS frame itself asked for,
            // to distinguish "only our own blink/content scheduling wants a
            // wake" from "something egui-internal also wants one sooner".
            win.pending_terminal_requested_delay = shortest_repaint_delay;

            // Schedule a repaint at the shortest interval needed by any pane.
            if let Some(delay) = shortest_repaint_delay {
                ctx.request_repaint_after(delay);
            }

            // Task 121 frame-profiling harness (defect-1/defect-2 fix):
            // derive this frame's `phase_orchestration` as `central_body`'s
            // total elapsed time minus `phase_panes_this_frame` (the
            // per-pane `show()` contribution accumulated above), and hand
            // both durations OUT via the captured `phase_orchestration_out`/
            // `phase_panes_out` locals declared before this closure. NO
            // accumulation into `win.frame_stats` and NO `tracing::debug!`
            // flush happens here anymore -- both moved to a single recording
            // point after `compose_with_chrome_damage`, near the end of
            // `update()`, because the `frame_damage_full`/`frame_damage_partial`
            // counters can only be correctly attributed from the FINAL,
            // post-composition `win.pending_frame_damage` value, which does
            // not exist until after this closure returns (on either the FULL
            // or REPLAY branch below).
            #[cfg(feature = "frame-profiling")]
            {
                let central_body_elapsed = central_body_start.elapsed();
                phase_orchestration_out =
                    central_body_elapsed.saturating_sub(phase_panes_this_frame);
                phase_panes_out = phase_panes_this_frame;
            }
        };

        if let Some(mut root_ui) = chrome_root_ui {
            let _panel_response = CentralPanel::default().show(&mut root_ui, central_body);
        } else {
            // REPLAY: construct the band's `Ui` directly at the cached
            // content rect, in the SAME background layer chrome uses, so the
            // terminal band's shapes land where a FULL frame's `CentralPanel`
            // content would have put them. The id is NOT the same, though:
            // this uses `Id::new("freminal_root")` directly (the root Ui's
            // own id), while the FULL path's `CentralPanel::show` allocates
            // its content `Ui` via `root_ui.new_child(..)` with no explicit
            // id salt, which egui auto-derives from `root_ui`'s id plus a
            // per-frame child-index counter — a different, and not
            // necessarily stable, id. This is a known accepted limitation:
            // any widget that keys persistent state off its `Ui`-derived id
            // (e.g. collapsing-header open state) could in principle churn
            // that state across a Full<->Replay mode toggle. In practice
            // this is inert, because real user interaction with such a
            // widget forces `ChromeMode::Full` on the same frame (via
            // `ui_overlay_open`/pointer-motion/etc.), so REPLAY is only ever
            // entered while nothing is interacting with mismatched-id
            // widgets. Tracked as a follow-up if a concrete widget is ever
            // found to rely on cross-mode id stability.
            // `decide_chrome_mode` only chooses `Replay` when the chrome
            // cache is valid at this frame's size/ppp, which is only ever
            // populated on a FULL frame — and every FULL frame publishes
            // `win.published`'s cached central rect (via `central_body`)
            // before that cache is populated — so falling back to egui's
            // own content rect (`cached_central_rect_for_replay`'s
            // fallback, computed above) should be unreachable in practice.
            let mut band_ui = egui::Ui::new(
                ctx.clone(),
                egui::Id::new("freminal_root"),
                egui::UiBuilder::new()
                    .layer_id(egui::LayerId::background())
                    .max_rect(cached_central_rect_for_replay),
            );
            central_body(&mut band_ui);
        }

        // Render the app-level toast stack as an overlay on top of all panels.
        // Toasts are shared across every window, so they appear consistently
        // regardless of which window the user is looking at. TAIL chrome
        // (#436.4b): skipped on REPLAY for the same reason as the dialogs
        // above — a toast being visible forces `ChromeDamage::Changed` every
        // frame it is (`ChromeSignals::toast_active`), so a REPLAY frame can
        // only ever be entered while the stack is provably empty, making
        // `.show()` a no-op here anyway.
        if chrome_mode == freminal_windowing::ChromeMode::Full {
            // Review item 2 follow-up to 121.14: `win.published`'s toast
            // rects must be written on EVERY `Full` frame reaching this
            // point, not only when the block below actually runs
            // `stack.show()` — see `PublishedFrameState`'s doc for why (an
            // emptied stack would otherwise leave stale rects behind
            // forever). Default to cleared; the inner block below
            // overwrites with fresh rects when it runs.
            win.published.clear_chrome_toast_rects();

            if let Ok(mut stack) = self.toasts.try_borrow_mut()
                && !stack.is_empty()
            {
                // Rebuild the same geometry `central_body` used to populate
                // `win.published`'s cached central rect / the pane layout —
                // `win` is a local variable here (removed from
                // `self.windows` above, reinserted below), so this cannot
                // reuse `central_body`'s own locals (`pane_layout`,
                // `available_rect`), which are scoped to that closure.
                let content_rect = win
                    .published
                    .cached_central_rect()
                    .unwrap_or_else(|| ctx.input(egui::InputState::content_rect));
                // Pre-resolve the active tab's pane layout into owned locals so
                // the `resolve_pane_rect` closure below does not borrow `win`
                // (it only captures `Copy`/owned data) — it needs to coexist
                // with the `win.terminal_widget.font_manager_mut()` borrow
                // passed alongside it to `stack.show`.
                let active_tab = win.tabs.active_tab();
                let zoomed_pane = active_tab.zoomed_pane;
                let pane_layout: Vec<(crate::gui::panes::PaneId, egui::Rect)> = active_tab
                    .pane_tree
                    .layout(geometry_interop::rect_from_egui(content_rect))
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(id, r)| (id, geometry_interop::rect_to_egui(r)))
                    .collect();
                let resolve_pane_rect =
                    move |pane_id: crate::gui::panes::PaneId| -> Option<egui::Rect> {
                        if let Some(zoomed_id) = zoomed_pane {
                            return (zoomed_id == pane_id).then_some(content_rect);
                        }
                        pane_layout
                            .iter()
                            .find(|(id, _)| *id == pane_id)
                            .map(|(_, r)| *r)
                    };
                let pixels_per_point = ctx.pixels_per_point();
                let resources = super::toast::ToastFrameResources {
                    render_state: &win.toast_render_state,
                    font_manager: win.terminal_widget.font_manager_mut(),
                };
                let outcome = stack.show(
                    ctx,
                    content_rect,
                    window_id,
                    resolve_pane_rect,
                    resources,
                    pixels_per_point,
                );

                // Subtask 121.14 (review item 2 follow-up): the laid-out
                // toast pill rects go into their own dedicated slot, not
                // `chrome_border_rects` — see `PublishedFrameState`'s doc
                // for the full staleness-discipline reasoning and why a
                // dedicated field replaced the original
                // append-to-border-rects approach.
                win.published.publish_chrome_toast_rects(outcome.rects);

                // Subtask 121.12: `ToastStack::show` returns its wanted delay
                // rather than calling `ctx.request_repaint_after()` itself. This
                // runs AFTER `central_body` already published
                // `win.pending_terminal_requested_delay` (~3768 above), so it is
                // a second aggregation point — the toast stack renders outside
                // `central_body`, once per window, after the per-window local
                // aggregate has already been folded and published. Fold it into
                // that published field too (so `chrome_repaint_settled`'s
                // suppressed-pointer / next-frame comparisons see it) and
                // schedule it directly, since `central_body`'s own scheduling
                // call has already run and will not run again this frame.
                if let Some(delay) = outcome.repaint_delay {
                    win.pending_terminal_requested_delay = Some(
                        win.pending_terminal_requested_delay
                            .map_or(delay, |prev| prev.min(delay)),
                    );
                    ctx.request_repaint_after(delay);
                }
            }
        }

        // ── Chrome-damage (#436.3): §3.5 "after" sample + final decision ─────
        //
        // Taken here — after every dismissible element's `.show(ctx)` this
        // frame, including the toast stack's above — and diffed against
        // `chrome_dismissible_before` (sampled at the very top of this
        // function, before any of them showed) to catch a self-dismissal
        // that happened DURING this frame's rendering (adversarial finding
        // 1: e.g. a toast expiring in its own `.show()` and requesting no
        // further repaint because the stack is now empty). Also diffed
        // against `win.prev_dismissible_presence` (last frame's own "after"
        // sample) to catch a transition caused by something other than the
        // element's own self-dismissal (e.g. a menu action closing a
        // dialog). Either comparison finding a difference counts as a
        // transition — see `chrome_damage::dismissible_presence_transitioned`'s
        // doc for why the intra-frame comparison is the load-bearing one.
        let chrome_dismissible_after = self.sample_dismissible_presence(&win);
        let chrome_presence_transitioned = chrome_damage::dismissible_presence_transitioned(
            chrome_dismissible_before,
            chrome_dismissible_after,
        ) || chrome_damage::dismissible_presence_transitioned(
            win.prev_dismissible_presence,
            chrome_dismissible_after,
        );
        win.prev_dismissible_presence = chrome_dismissible_after;

        // §3.5's "+ next frame FULL" half: read last frame's pending flag as
        // this frame's settle input, then reassign it to THIS frame's own
        // transition result for the next frame to read (no separate "reset"
        // step — see `decide_chrome_damage`'s doc).
        let chrome_settle_frame_pending = win.chrome_settle_pending;
        win.chrome_settle_pending = chrome_presence_transitioned;

        let staged_chrome_signals = win.published.pending_chrome_signals();
        win.pending_chrome_damage = chrome_damage::decide_chrome_damage(
            &staged_chrome_signals,
            chrome_presence_transitioned,
            chrome_settle_frame_pending,
        );

        // ── #435/#436 composition (§6): chrome change forces FrameDamage::Full ──
        //
        // The #435 partial-present decision (`pending_frame_damage`, computed
        // in `central_body`) and the #436 chrome-cache decision
        // (`pending_chrome_damage`, just computed) are separate but MUST
        // agree. See `frame_damage::compose_with_chrome_damage` for the full
        // rationale; in short, a frame that changed chrome pixels must not be
        // presented `Partial` (it would leave chrome outside the cursor rect
        // stale under #435's `buffer_age() == 1` assumption). Reconciled here,
        // after both decisions are final, via the pure helper.
        win.pending_frame_damage = frame_damage::compose_with_chrome_damage(
            std::mem::replace(
                &mut win.pending_frame_damage,
                freminal_windowing::FrameDamage::Full,
            ),
            win.pending_chrome_damage,
        );

        // Task 121 frame-profiling harness: single recording point covering
        // the defect-1 (`phase_app_update`), defect-2 (`frame_damage_full`/
        // `frame_damage_partial` counted from the FINAL value), and
        // defect-3 (`window_id` on the tracing line) fixes.
        //
        // Placed here -- after `compose_with_chrome_damage` and before `win`
        // is reinserted into `self.windows` -- rather than inside
        // `central_body`, for two reasons: (1) `win.pending_frame_damage` is
        // only final as of the composition just above (`central_body` only
        // ever sees the PRE-composition value); (2) `update_start.elapsed()`
        // must be taken as late as possible in the function's productive
        // body to actually cover the menu/tab-bar construction, dead-pane
        // cleanup / session-autosave / settings dispatch, the toast stack's
        // `.show()`, and the chrome-damage after-sample + decide + compose
        // steps -- all of which run OUTSIDE `central_body` but were
        // previously (wrongly) excluded from every app-side phase
        // measurement and therefore misattributed to "egui overhead".
        #[cfg(feature = "frame-profiling")]
        {
            // Defect 2: count the FINAL, post-composition frame damage kind.
            match &win.pending_frame_damage {
                freminal_windowing::FrameDamage::Full => {
                    win.frame_stats.frame_damage_full =
                        win.frame_stats.frame_damage_full.saturating_add(1);
                }
                freminal_windowing::FrameDamage::Partial(_) => {
                    win.frame_stats.frame_damage_partial =
                        win.frame_stats.frame_damage_partial.saturating_add(1);
                }
            }

            // Defect 1: `phase_app_update` covers the whole productive body
            // of `update()`, from `update_start` (captured at the very top
            // of this function) through this point.
            let phase_app_update_this_frame = update_start.elapsed();

            let stats = &mut win.frame_stats;
            stats.phase_orchestration_total += phase_orchestration_out;
            stats.phase_orchestration_max =
                stats.phase_orchestration_max.max(phase_orchestration_out);
            stats.phase_panes_total += phase_panes_out;
            stats.phase_panes_max = stats.phase_panes_max.max(phase_panes_out);
            stats.phase_app_update_total += phase_app_update_this_frame;
            stats.phase_app_update_max =
                stats.phase_app_update_max.max(phase_app_update_this_frame);

            if stats
                .frames_drawn
                .is_multiple_of(super::window::FrameStats::FLUSH_EVERY)
            {
                // Defect 3: `window_id` on the tracing line, `{:?}` -- the
                // same representation the windowing crate's own
                // `frame_profiling` line uses for its `window_id` field, so
                // the two crates' lines for the same OS window can be
                // matched by eye or by log-processing tooling.
                tracing::debug!(
                    target: "freminal::frame_profiling",
                    window_id = ?window_id,
                    frames_drawn = stats.frames_drawn,
                    chrome_mode_full = stats.chrome_mode_full,
                    chrome_mode_replay = stats.chrome_mode_replay,
                    chrome_replay_duty_cycle_pct =
                        super::window::FrameStats::chrome_replay_duty_cycle_pct(
                            stats.chrome_mode_full,
                            stats.chrome_mode_replay
                        ),
                    zero_change_presented = stats.zero_change_presented,
                    frame_damage_full = stats.frame_damage_full,
                    frame_damage_partial = stats.frame_damage_partial,
                    phase_app_update_total_us = stats.phase_app_update_total.as_micros(),
                    phase_app_update_max_us = stats.phase_app_update_max.as_micros(),
                    phase_app_update_mean_us = super::window::FrameStats::mean_duration(
                        stats.phase_app_update_total,
                        stats.frames_drawn
                    )
                    .as_micros(),
                    phase_orchestration_total_us = stats.phase_orchestration_total.as_micros(),
                    phase_orchestration_max_us = stats.phase_orchestration_max.as_micros(),
                    phase_orchestration_mean_us = super::window::FrameStats::mean_duration(
                        stats.phase_orchestration_total,
                        stats.frames_drawn
                    )
                    .as_micros(),
                    phase_panes_total_us = stats.phase_panes_total.as_micros(),
                    phase_panes_max_us = stats.phase_panes_max.as_micros(),
                    phase_panes_mean_us = super::window::FrameStats::mean_duration(
                        stats.phase_panes_total,
                        stats.frames_drawn
                    )
                    .as_micros(),
                    // Gate-blocker investigation (issue #459/#461 follow-up):
                    // which individual §3.3 `ChromeSignals` field(s) fired,
                    // cumulative since window creation, name=count joined --
                    // only the non-zero entries (see
                    // `format_nonzero_counts`'s doc for why: 15
                    // separate structured fields would be unreadable on the
                    // common case where only 1-2 signals ever fire, e.g. an
                    // idle blinking cursor should show "none" here every
                    // flush once past warm-up).
                    chrome_signals_fired = %super::window::FrameStats::format_nonzero_counts(
                        &std::array::from_fn::<_, 15, _>(|i| {
                            (
                                chrome_damage::ChromeSignals::default().named_fields()[i].0,
                                stats.chrome_signal_fired_counts[i],
                            )
                        })
                    ),
                    // Task 121 pointer-motion repaint-gate spike follow-up:
                    // of the `pointer_motion_needs_repaint` calls this flush
                    // window, how many total, and which of the eight named
                    // conditions fired on how many of them (non-exclusive --
                    // several can fire on the same call; each counted
                    // independently, see `record_pointer_motion_check`).
                    // WINDOWED (reset below), unlike `chrome_signals_fired`
                    // above -- see `pointer_repaint_check_total`'s field doc
                    // in `window.rs` for why.
                    pointer_repaint_checks_total = stats.pointer_repaint_check_total.get(),
                    pointer_repaint_conditions_fired =
                        %super::window::FrameStats::format_nonzero_counts(
                            &stats.pointer_condition_counts()
                        ),
                    "app-side frame-profiling stats (task 121 harness): chrome-mode \
                     duty cycle, zero-pixel-change-but-presented frames, the \
                     freminal-owned phase_app_update/phase_orchestration/phase_panes \
                     wall-clock split (phase_app_update = the whole productive body of \
                     update(); phase_orchestration = central_body total minus the \
                     per-pane show() contribution), which individual §3.3 \
                     ChromeSignals field(s) fired (chrome_signals_fired, cumulative, \
                     non-zero entries only) over frames_drawn drawn frames for this \
                     window_id, and -- the pointer-motion repaint-gate spike \
                     follow-up -- how many of the last pointer_repaint_checks_total \
                     `pointer_motion_needs_repaint` calls each of the eight named \
                     gate conditions fired on this flush window \
                     (pointer_repaint_conditions_fired, non-zero entries only, \
                     reset every flush window)"
                );

                // Windowed, not cumulative-since-creation (see the field
                // doc) -- clear now that this window's line has been
                // logged. `chrome_signal_fired_counts` above is NOT reset;
                // it stays cumulative like every other plain `FrameStats`
                // counter.
                stats.reset_pointer_condition_window();
            }
        }

        let elapsed = now.elapsed();
        let frame_time = if elapsed.as_millis() > 0 {
            format!("Frame time={}ms", elapsed.as_millis())
        } else {
            format!("Frame time={}μs", elapsed.as_micros())
        };

        trace!("{}", frame_time);

        // Reinsert per-window state before returning.
        self.windows.insert(window_id, win);

        // Apply a pending layout (set from the Layouts menu).
        if let Some(resolved) = self.pending_load_layout.take() {
            let commands = self.apply_layout(&resolved, window_id, handle);
            self.inject_layout_commands(&commands);
        }
    }

    fn raw_input_hook(&mut self, _window_id: WindowId, raw_input: &mut egui::RawInput) {
        // Override egui's predicted frame time to zero.
        //
        // egui's `request_repaint_after(delay)` subtracts `predicted_dt`
        // (~16.7 ms at the default 1/60) from the requested delay to avoid
        // "overshooting" into the next frame.  With vsync disabled (see the
        // `native_options.vsync = false` below), this subtraction collapses
        // any delay ≤ 16.7 ms to zero — turning every repaint request into
        // an immediate repaint and driving the frame rate to hundreds of FPS
        // during active PTY output.
        //
        // Setting `predicted_dt = 0` disables the subtraction, so our delays
        // are honoured exactly:
        //   - 8 ms  (PTY thread after each batch)  → ~120 FPS cap
        //   - 16 ms (GUI on content_changed)        → ~60 FPS cap
        //   - 500 ms (cursor blink)                 → ~2 FPS
        //   - no request (true idle, steady cursor)  → 0 FPS
        raw_input.predicted_dt = 0.0;
    }

    fn on_raw_key_event(
        &mut self,
        window_id: WindowId,
        event: freminal_windowing::RawKeyEvent,
        mods: freminal_windowing::RawKeyMods,
    ) {
        // Task 114.7: queue only -- do NOT encode here. This callback fires
        // at winit-event time, outside the render/`update()` path where the
        // active pane, its snapshot, and the true per-pane `super_pressed`
        // are in scope. The queue is drained and encoded on the render path
        // in `update()` (see `terminal::input::drain_pending_raw_keys`).
        //
        // No explicit repaint request is needed here: `event_loop.rs`'s
        // `KeyboardInput` intercept already sets `state.repaint_at =
        // Some(Instant::now())` immediately after calling this method, so a
        // repaint (and therefore a drain) is already guaranteed this cycle.
        let Some(win) = self.windows.get_mut(&window_id) else {
            trace!(?window_id, "raw key event for unknown window; dropping");
            return;
        };
        win.pending_raw_keys.push((event, mods));
    }
}

impl FreminalGui {
    /// Sample the presence of every dismissible chrome element (#436 §3.5).
    ///
    /// Called twice per `update()` for the window being rendered — once as
    /// early as possible (before any dialog's `.show(ctx)` this frame) and
    /// once after all of them (including the shared toast stack's `.show`,
    /// which runs against the still-local `win` before it is reinserted into
    /// `self.windows` at the end of `update()` — see the call site) — so the
    /// two samples can be diffed to catch a self-dismissal that happens
    /// DURING a `.show()` call this same frame (adversarial finding 1).
    fn sample_dismissible_presence(
        &self,
        win: &PerWindowState,
    ) -> chrome_damage::DismissiblePresence {
        chrome_damage::DismissiblePresence {
            about: self.about_window_open,
            welcome: self.welcome.is_open(),
            paste_dialog: win.paste_dialog.is_open(),
            broadcast_dialog: win.broadcast_dialog.is_open(),
            close_dialog: win.close_dialog.is_open(),
            save_layout_prompt: self.pending_save_layout.is_some(),
            any_toast: self
                .toasts
                .try_borrow()
                .is_ok_and(|stack| !stack.is_empty()),
        }
    }

    /// Route the OSC 9/777, OSC 52, and OSC 99 events that
    /// [`drain_window_manipulation_commands`] collected during its per-pane
    /// drain: OSC 9/777 notifications go through
    /// [`crate::gui::notifications::NotificationRouter::route`], OSC 52
    /// clipboard events surface as a toast via [`Self::route_freminal_toast`],
    /// OSC 99 stateful notifications go through
    /// [`crate::gui::notifications::NotificationRouter::route_osc99`], and
    /// OSC 99 control sequences (alive/close/query) get PTY-written
    /// responses.
    ///
    /// Called after `drain_window_manipulation_commands` returns, i.e. after
    /// its mutable borrow of the tab list has ended, so `self.config`,
    /// `self.toasts`, and the OSC 99 session maps (`self.osc99_icon_cache`,
    /// `self.osc99_live`) are all borrowable here without conflicting with
    /// that borrow.
    ///
    /// The four routing destinations are kept as four separate blocks
    /// deliberately (Task 122.8 prohibition): they route to different
    /// destinations, and the OSC 99 control block writes PTY responses via
    /// `send_or_log!`. Do not fold them into one.
    fn route_window_manipulation_events(
        &self,
        ui: &egui::Ui,
        window_focus: WindowFocus,
        events: &WindowManipulationEvents,
    ) {
        // Route OSC 9 / OSC 777 notifications collected above (Task 76.4).
        // `self.config` and the toast stack are borrowable here without
        // conflicting with the drain's tab-list borrow.
        if !events.osc_notifications.is_empty()
            && let Ok(mut toasts) = self.toasts.try_borrow_mut()
        {
            for req in &events.osc_notifications {
                crate::gui::notifications::NotificationRouter::route(
                    req,
                    &self.config.notifications,
                    window_focus.is_focused(),
                    &mut toasts,
                );
            }
        }

        // Surface OSC 52 remote-clipboard events as toasts (issue #433).
        for event in &events.osc52_events {
            let (title, detail) = rendering::osc52_toast_text(event);
            self.route_freminal_toast(
                freminal_common::config::FreminalToastCategory::ClipboardRemote,
                crate::gui::toast::ToastKind::Info,
                title,
                detail,
                crate::gui::toast::ToastPlacement::WINDOW_CENTERED,
            );
        }

        // Route OSC 99 stateful notifications collected above (Task 99.5a).
        // `self.config`, the toast stack, and the OSC 99 session maps are
        // borrowable here without conflicting with the drain's tab-list
        // borrow.
        if !events.osc99_notifications.is_empty() {
            let window_minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
            if let (Ok(mut toasts), Ok(mut icon_cache), Ok(mut live)) = (
                self.toasts.try_borrow_mut(),
                self.osc99_icon_cache.try_borrow_mut(),
                self.osc99_live.try_borrow_mut(),
            ) {
                let ctx = crate::gui::notifications::Osc99DisplayContext {
                    window_focused: window_focus.is_focused(),
                    window_minimized,
                };
                // `tx` (the originating pane's `pty_write_tx` clone) is
                // threaded into the reverse-write path (Task 99.6): the
                // notification thread uses it to write activation/close
                // reports back to the pane that produced this OSC 99
                // sequence.
                for (data, tx) in &events.osc99_notifications {
                    crate::gui::notifications::NotificationRouter::route_osc99(
                        data,
                        &self.config.notifications,
                        ctx,
                        &mut toasts,
                        &mut icon_cache,
                        &mut live,
                        tx,
                    );
                }
            }
        }

        // Answer OSC 99 control sequences collected above (Task 99.6 for
        // Close/Alive; the Query capability handshake is Task 99.7). Run
        // after the display-routing block above so its `osc99_live` borrow
        // has already been released.
        for (control, tx) in &events.osc99_controls {
            match control.kind {
                Osc99ControlKind::Alive => {
                    // Answer the poll with the current live notification ids.
                    if let Ok(live) = self.osc99_live.try_borrow() {
                        let ids = crate::gui::notifications::live_ids_sorted(&live);
                        let bytes = crate::gui::notifications::osc99_alive_report(
                            control.id.as_deref(),
                            &ids,
                        );
                        send_or_log!(
                            tx,
                            PtyWrite::Write(bytes),
                            "Failed to send OSC 99 alive report"
                        );
                    }
                }
                Osc99ControlKind::Close => {
                    // App-driven close request: prune the live entry.
                    // freminal cannot programmatically close an OS
                    // notification it already delegated to the desktop
                    // environment, so this only reconciles our liveness
                    // map — no report is sent here (the close report is
                    // emitted only when WE observe a close on the
                    // notification thread with `c=1`).
                    if let (Some(id), Ok(mut live)) =
                        (control.id.as_deref(), self.osc99_live.try_borrow_mut())
                    {
                        crate::gui::notifications::forget_osc99(&mut live, id);
                    }
                }
                Osc99ControlKind::Query => {
                    // OSC 99 p=? capability handshake (Task 99.7): answer
                    // with freminal's truthfully-advertised OSC 99
                    // capabilities.
                    let bytes =
                        crate::gui::notifications::osc99_query_response(control.id.as_deref());
                    send_or_log!(
                        tx,
                        PtyWrite::Write(bytes),
                        "Failed to send OSC 99 capability response"
                    );
                }
            }
        }
    }

    /// First-window spawn path when no layout or session restore will apply.
    ///
    /// Spawns a default single-pane PTY.  PTY-spawn failures surface as a
    /// user-visible toast (the window still opens, empty) rather than
    /// aborting the application.  This mirrors the subsequent-window
    /// branch's error handling.
    #[allow(clippy::too_many_arguments)] // Helper inherits all of on_window_created's context.
    fn create_first_window_with_default_pty(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
        os_dark_mode: bool,
        repaint_handle: Arc<std::sync::OnceLock<(freminal_windowing::RepaintProxy, WindowId)>>,
        window_post: Arc<Mutex<WindowPostRenderer>>,
    ) {
        let proxy = handle.event_loop_proxy();
        let _ = repaint_handle.set((proxy, window_id));

        let theme = freminal_common::themes::by_slug(self.config.theme.active_slug(os_dark_mode))
            .unwrap_or(&freminal_common::themes::CATPPUCCIN_MOCHA);
        rendering::set_egui_options(
            ctx,
            theme,
            self.config.ui.background_opacity,
            &self.gui_theme,
        );

        let terminal_widget = FreminalTerminalWidget::new(ctx, &self.config).unwrap_or_else(|e| {
            tracing::error!("fatal: failed to initialise terminal widget (font manager): {e}");
            std::process::exit(1);
        });
        let (cell_w, cell_h) = terminal_widget.cell_size();
        let initial_size = Self::compute_initial_size(inner_size.0, inner_size.1, cell_w, cell_h);

        let pane_id = self
            .pane_id_gen
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .next_id();

        let channels = match super::pty::spawn_pty_tab(
            &self.args,
            self.config.scrollback.limit,
            super::pty::PtyTabInitialState {
                theme,
                auto_detect_urls: self.config.ui.auto_detect_urls,
                cursor_style: freminal_common::cursor::CursorVisualStyle::from_config(
                    &self.config.cursor.shape,
                    self.config.cursor.blink,
                ),
            },
            &repaint_handle,
            initial_size,
            super::pty::PtyTabConfig {
                cwd: None,
                shell_override: None,
                extra_env: None,
                recording_swap: self.recording_swap.clone(),
                recording_pane_id: pane_id.raw().try_into().unwrap_or(u32::MAX),
                set_term_program: self.config.shell_integration.set_term_program,
            },
        ) {
            Ok(channels) => channels,
            Err(e) => {
                error!("Failed to spawn initial PTY: {e}");
                // This is the first/only window and it has no other live
                // panes — a failed spawn here is fatal.  Record it so the
                // window renders the fatal-error panel (with an Exit button)
                // instead of a blank surface.  We deliberately do NOT close
                // the window: closing the only window would quit the app
                // before the user can read why.
                self.set_fatal_error(
                    "Failed to start shell",
                    format!("The shell could not be started:\n\n{e}"),
                );
                return;
            }
        };

        let pane = panes::Pane::from_channels(
            pane_id,
            channels,
            Arc::clone(&window_post),
            "Terminal".to_owned(),
        );

        let tab = Tab::new(super::tabs::TabId::first(), pane);

        // Inform the initial tab about the configured theme mode and real
        // OS dark/light preference so DECRPM ?2031 responses are correct.
        if let Some(active) = tab.active_pane()
            && let Err(e) =
                active
                    .input_tx
                    .send(freminal_terminal_emulator::io::InputEvent::ThemeModeUpdate(
                        self.config.theme.mode,
                        os_dark_mode,
                    ))
        {
            error!("Failed to send ThemeModeUpdate to initial tab: {e}");
        }

        // Apply initial background image from config (if set).
        let initial_bg_path = self.config.ui.background_image.clone();
        if initial_bg_path.is_some()
            && let Ok(panes_list) = tab.pane_tree.iter_panes()
        {
            for p in panes_list {
                if let Ok(mut rs) = p.render_state.lock() {
                    rs.set_pending_bg_image(initial_bg_path.clone());
                }
            }
        }

        let win = Self::new_per_window_state(
            tab,
            terminal_widget,
            os_dark_mode,
            window_post,
            repaint_handle,
        );
        self.windows.insert(window_id, win);
    }

    /// Render the fatal-error panel for a window that has no
    /// [`PerWindowState`] because the only/last shell failed to spawn.
    ///
    /// Shows the stored title, the underlying error detail, and a single
    /// "Exit" button that quits the application.  Replaces what would
    /// otherwise be a blank, unrecoverable window.
    fn render_fatal_error(&self, ctx: &egui::Context) {
        let Some((title, detail)) = self.fatal_error.as_ref() else {
            return;
        };
        // Match the rest of the GUI: build a root Ui covering the window and
        // reserve space from it via `show` (the non-deprecated API; `show_inside`
        // was renamed to `show` in egui 0.35).
        let mut root_ui = egui::Ui::new(
            ctx.clone(),
            egui::Id::new("freminal_fatal_error_root"),
            egui::UiBuilder::default(),
        );
        CentralPanel::default().show(&mut root_ui, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(48.0);
                ui.heading(title);
                ui.add_space(12.0);
                ui.label(detail);
                ui.add_space(24.0);
                if ui.button("Exit").clicked() {
                    std::process::exit(1);
                }
            });
        });
    }

    /// Construct a `PerWindowState` with default field values for all
    /// transient UI state.  Extracted to keep
    /// `create_first_window_with_default_pty` under the line limit.
    fn new_per_window_state(
        tab: Tab,
        terminal_widget: FreminalTerminalWidget,
        os_dark_mode: bool,
        window_post: Arc<Mutex<WindowPostRenderer>>,
        repaint_handle: Arc<std::sync::OnceLock<(freminal_windowing::RepaintProxy, WindowId)>>,
    ) -> PerWindowState {
        PerWindowState {
            tabs: TabManager::new(tab),
            terminal_widget,
            last_window_title: String::from("Freminal"),
            os_dark_mode,
            style_cache: None,
            pending_close_pane: false,
            pending_focus_direction: None,
            border_drag: None,
            published: super::published_frame_state::PublishedFrameState::new(),
            shader_last_mtime: None,
            window_post,
            toast_render_state: crate::gui::renderer::ToastRenderState::new_shared(),
            repaint_handle,
            pending_new_window: false,
            pending_geometry: None,
            last_known_size: None,
            last_known_position: None,
            renaming_tab: None,
            rename_buffer: String::new(),
            dragging_tab: None,
            last_tab_rects: Vec::new(),
            pending_menu_actions: Vec::new(),
            paste_dialog: super::paste_guard::PasteDialog::default(),
            broadcast_dialog: super::broadcast_guard::BroadcastConfirmDialog::default(),
            close_dialog: super::close_guard::CloseGuardDialog::default(),
            pending_force_close: false,
            pending_raw_keys: Vec::new(),
            pending_frame_damage: freminal_windowing::FrameDamage::Full,
            pending_terminal_band_range: 0..0,
            present_is_partial: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            previous_active_pane_key: None,
            pending_chrome_damage: freminal_windowing::ChromeDamage::Changed,
            chrome_settle_pending: false,
            prev_dismissible_presence: chrome_damage::DismissiblePresence::default(),
            prev_chrome_tab_snapshot: chrome_damage::ChromeTabSnapshot::default(),
            prev_window_focused: false,
            chrome_frames_rendered: 0,
            pending_terminal_requested_delay: None,
            frame_stats: super::window::FrameStats::default(),
        }
    }

    /// First-window spawn path when a startup layout or session restore
    /// will populate the window's tabs.
    ///
    /// Resolves the layout (from `--layout`, `startup.layout`, or
    /// `last_session.toml`), pushes every resolved window into
    /// `pending_layout_windows`, builds the first `PerWindowState` by
    /// popping the first entry, and creates OS windows for the rest.
    ///
    /// If resolution fails, pushes an error toast and falls back to
    /// spawning a default PTY so the user still gets a usable terminal.
    #[allow(clippy::too_many_arguments)] // Helper inherits all of on_window_created's context.
    fn create_first_window_from_layout_or_restore(
        &mut self,
        window_id: WindowId,
        ctx: &egui::Context,
        handle: &freminal_windowing::WindowHandle<'_>,
        inner_size: (u32, u32),
        os_dark_mode: bool,
        repaint_handle: Arc<std::sync::OnceLock<(freminal_windowing::RepaintProxy, WindowId)>>,
        window_post: Arc<Mutex<WindowPostRenderer>>,
    ) {
        let Some(resolved) = self.resolve_startup_layout_or_session() else {
            // Resolution failed and a toast was already pushed.  Fall
            // back to a default PTY so the window is still useful.
            self.create_first_window_with_default_pty(
                window_id,
                ctx,
                handle,
                inner_size,
                os_dark_mode,
                repaint_handle,
                window_post,
            );
            return;
        };

        // Queue all resolved windows.  The first is consumed below for
        // this window; subsequent ones trigger fresh
        // `on_window_created` callbacks that will pop and build their
        // own `PerWindowState`.
        for w in &resolved.windows {
            self.pending_layout_windows.push_back(w.clone());
        }

        // Build this first window by popping the first queued entry.
        let cmds_opt = self.build_window_from_pending_layout(
            window_id,
            ctx,
            handle,
            inner_size,
            os_dark_mode,
            Some((repaint_handle, window_post)),
        );

        // Create OS windows for any remaining pending layout windows.
        // Their sizes/positions are taken from the layout.
        let remaining: Vec<_> = self.pending_layout_windows.iter().cloned().collect();
        for extra_window in remaining {
            handle.create_window(freminal_windowing::WindowConfig {
                title: "Freminal".to_owned(),
                inner_size: extra_window.size.map(<[u32; 2]>::into),
                position: extra_window.position.map(<[i32; 2]>::into),
                transparent: true,
                icon: self.icon.clone(),
                app_id: Some("freminal".into()),
            });
        }

        if let Some(cmds) = cmds_opt {
            self.inject_layout_commands(&cmds);
        } else if !self.has_live_window() {
            // The first window's tabs could not be built (every pane spawn
            // failed) and no other window holds a live pane.  Without this
            // the window would be left blank and unrecoverable.  Record a
            // fatal error so the next frame renders the Exit panel.  A more
            // specific per-pane spawn error has already been surfaced as a
            // toast by `spawn_pane_from_leaf`; this is the catch-all that
            // guarantees a visible, actionable failure state.
            self.set_fatal_error(
                "Failed to start session",
                "No shell could be started for the restored session or \
                 layout.\n\nThis usually means the shell program could not \
                 be launched. Check your shell configuration, or try \
                 launching with shell integration disabled \
                 ([shell_integration] set_term_program = false).",
            );
        }
    }

    /// Resolve the startup layout or session-restore source to a
    /// `ResolvedLayout`, if any applies.
    ///
    /// Tries in priority order:
    /// 1. `--layout` CLI flag
    /// 2. `startup.layout` in config
    /// 3. `last_session.toml` when `startup.restore_last_session` is on
    ///    and no positional command was supplied.
    ///
    /// Returns `None` if no source applies or if loading/resolution
    /// fails.  On failure, pushes an error toast so the caller can fall
    /// back to a default PTY.
    fn resolve_startup_layout_or_session(&self) -> Option<freminal_common::layout::ResolvedLayout> {
        // Priority 1 + 2: --layout / startup.layout.
        if let Some(name_or_path) = self
            .args
            .layout
            .clone()
            .or_else(|| self.config.startup.layout.clone())
        {
            let path = Self::resolve_startup_layout_path(&name_or_path);
            let positional: Vec<String> = self
                .args
                .layout_vars
                .iter()
                .filter(|s| !s.contains('='))
                .cloned()
                .collect();
            let var_map = self.args.layout_var_map();
            return match freminal_common::layout::Layout::from_file(&path) {
                Ok(layout) => match layout.apply_variables(&positional, &var_map).resolve() {
                    Ok(resolved) if resolved.windows.is_empty() => {
                        // A structurally-valid but empty layout (no windows /
                        // no panes) cannot produce a usable window.  Treat it
                        // as "no layout applies" so the caller falls back to a
                        // default shell rather than rendering a blank/fatal
                        // window.
                        error!("Layout '{}' contains no windows", path.display());
                        self.push_error_toast(
                            "Layout is empty",
                            Some(format!(
                                "{} defines no windows or panes; starting a default shell.",
                                path.display()
                            )),
                        );
                        None
                    }
                    Ok(resolved) => Some(resolved),
                    Err(e) => {
                        error!("Failed to resolve layout '{}': {e}", path.display());
                        self.push_error_toast(
                            "Failed to resolve layout",
                            Some(format!("{}: {e}", path.display())),
                        );
                        None
                    }
                },
                Err(e) => {
                    error!("Failed to load layout '{}': {e}", path.display());
                    self.push_error_toast(
                        "Failed to load layout",
                        Some(format!("{}: {e}", path.display())),
                    );
                    None
                }
            };
        }

        // Priority 3: session restore.
        let path = Self::last_session_path()?;
        if !path.exists() {
            return None;
        }
        match freminal_common::layout::Layout::from_file(&path).and_then(|l| {
            l.apply_variables(&[], &std::collections::HashMap::new())
                .resolve()
        }) {
            // A blank or zero-window `last_session.toml` deserializes to a
            // structurally-valid but empty `Layout` (every field is
            // `#[serde(default)]`), so parsing *succeeds* and we land here with
            // no windows.  This is exactly the corruption case observed when a
            // previous run was killed mid-write (e.g. an aggressive reboot
            // truncated the file): without this guard the empty layout produced
            // a blank window and a fatal-error panel at startup.  Treat it like
            // a parse failure — warn the user via a non-blocking toast and fall
            // back to a default shell so the terminal still starts.
            Ok(resolved) if resolved.windows.is_empty() => {
                error!(
                    "restore_last_session: {} contains no windows (blank/corrupt session)",
                    path.display()
                );
                self.push_error_toast(
                    "Could not restore last session",
                    Some(
                        "The saved session was empty or corrupt; starting a default shell."
                            .to_owned(),
                    ),
                );
                None
            }
            Ok(resolved) => Some(resolved),
            Err(e) => {
                error!(
                    "restore_last_session: failed to apply {}: {e}",
                    path.display()
                );
                self.push_error_toast(
                    "Failed to restore last session",
                    Some(format!("{}: {e}", path.display())),
                );
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SettingsOwnerCloseDecision, cursor_blink_wants_repaint, pointer_forces_full_present,
        settings_owner_close_decision,
    };
    use crate::gui::frame_damage::{self, PaneDamageInput};
    use crate::gui::renderer::{CursorDamage, PaneFrameDamage};
    use freminal_common::cursor::CursorVisualStyle;

    #[test]
    fn blink_cursor_wants_repaint_only_when_actually_visible() {
        // Visible, active, echo on, blink style -> wants the ~500ms wake.
        assert!(cursor_blink_wants_repaint(
            &CursorVisualStyle::BlockCursorBlink,
            true,
            true,
            false
        ));
        // DECTCEM-hidden (show_cursor false) -> NO wake. This is the btop /
        // full-screen-TUI case: the fix. Style is still blink, but the
        // cursor is hidden, so we must not wake ~2x/sec.
        assert!(!cursor_blink_wants_repaint(
            &CursorVisualStyle::BlockCursorBlink,
            false,
            true,
            false
        ));
        // Not the active pane -> no wake (only the active pane draws a cursor).
        assert!(!cursor_blink_wants_repaint(
            &CursorVisualStyle::BlockCursorBlink,
            true,
            false,
            false
        ));
        // Password echo-off hides the cursor -> no wake.
        assert!(!cursor_blink_wants_repaint(
            &CursorVisualStyle::BlockCursorBlink,
            true,
            true,
            true
        ));
    }

    #[test]
    fn non_blink_cursor_never_wants_blink_repaint() {
        // A steady (non-blink) cursor never needs the periodic wake, even
        // when fully visible.
        for style in [
            CursorVisualStyle::BlockCursorSteady,
            CursorVisualStyle::UnderlineCursorSteady,
            CursorVisualStyle::VerticalLineCursorSteady,
        ] {
            assert!(
                !cursor_blink_wants_repaint(&style, true, true, false),
                "steady style {style:?} must not request a blink wake"
            );
        }
    }

    #[test]
    fn each_blink_style_variant_wants_repaint_when_visible() {
        for style in [
            CursorVisualStyle::BlockCursorBlink,
            CursorVisualStyle::UnderlineCursorBlink,
            CursorVisualStyle::VerticalLineCursorBlink,
        ] {
            assert!(
                cursor_blink_wants_repaint(&style, true, true, false),
                "blink style {style:?} must request a wake when visible"
            );
        }
    }

    #[test]
    fn pointer_forces_full_present_truth_table() {
        // Moving + over chrome -> force Full (hover tint changed).
        assert!(pointer_forces_full_present(true, true, false));
        // Moving + border drag latched -> force Full (drag may have moved the
        // pointer off the ±3px sensor mid-drag).
        assert!(pointer_forces_full_present(true, false, true));
        // Moving over plain terminal content, no drag -> no forced Full; the
        // terminal band tracks its own hover damage separately.
        assert!(!pointer_forces_full_present(true, false, false));
        // Not moving, over chrome -> no forced Full (nothing changed).
        assert!(!pointer_forces_full_present(false, true, false));
        // Not moving, border drag latched -> no forced Full (drag state alone
        // is not motion; `border_drag_active` only matters combined with
        // actual pointer movement).
        assert!(!pointer_forces_full_present(false, false, true));
        // Not moving, neither chrome nor drag -> no forced Full.
        assert!(!pointer_forces_full_present(false, false, false));
    }

    /// Pins the `stage_frame_damage` double-write contract (see that
    /// function's doc): the #435 pre-composition `FrameDamage` decided by
    /// `decide_frame_damage` (write #1, assigned to
    /// `win.pending_frame_damage` at the `stage_frame_damage` call site) and
    /// the #436 post-composition value produced by
    /// `compose_with_chrome_damage` (write #2, applied after `central_body`
    /// returns) must be able to disagree — that disagreement is the entire
    /// reason the second write exists (a chrome-changed frame must not be
    /// presented `Partial` even if #435's signals alone said so). If the two
    /// writes were ever collapsed into one, this frame's pre-composition
    /// `Partial` and post-composition `Full` would silently converge and
    /// this assertion would start failing loudly instead of the bug
    /// regressing silently.
    #[test]
    fn pending_frame_damage_double_write_stays_distinct() {
        // One pane took the cursor-only fast path with a real damage rect,
        // no bell, no force-full/toast override -> `decide_frame_damage`
        // (write #1) resolves to `Partial`.
        let per_pane_damage = [PaneDamageInput {
            bell_active: false,
            cursor_damage: PaneFrameDamage::CursorOnly(Some(CursorDamage {
                x: 0,
                y: 0,
                width: 8,
                height: 16,
            })),
        }];
        let pre_composition = frame_damage::decide_frame_damage(false, false, &per_pane_damage);
        assert!(
            matches!(pre_composition, freminal_windowing::FrameDamage::Partial(_)),
            "expected write #1 (pre-composition) to be Partial, got {pre_composition:?}"
        );

        // The #436 chrome-cache decision independently found chrome pixels
        // changed this same frame (e.g. a hover tint moved) -> write #2
        // upgrades the pre-composition `Partial` to `Full`.
        let post_composition = frame_damage::compose_with_chrome_damage(
            pre_composition,
            freminal_windowing::ChromeDamage::Changed,
        );
        assert!(
            matches!(post_composition, freminal_windowing::FrameDamage::Full),
            "expected write #2 (post-composition) to be Full, got {post_composition:?}"
        );
    }

    #[test]
    fn not_owner_ignores_other_state() {
        assert_eq!(
            settings_owner_close_decision(false, false),
            SettingsOwnerCloseDecision::NotOwner
        );
        assert_eq!(
            settings_owner_close_decision(false, true),
            SettingsOwnerCloseDecision::NotOwner,
            "a non-owner window closing must never touch the settings guard, \
             even if `has_unsaved` happens to be set for some other window"
        );
    }

    #[test]
    fn clean_owner_closes_now() {
        assert_eq!(
            settings_owner_close_decision(true, false),
            SettingsOwnerCloseDecision::CloseNow
        );
    }

    #[test]
    fn dirty_owner_is_vetoed_with_prompt() {
        assert_eq!(
            settings_owner_close_decision(true, true),
            SettingsOwnerCloseDecision::VetoWithPrompt
        );
    }

    // ── Terminal band shape-index-range extraction (#436.2a / #436.4a) ──
    //
    // These tests pin the underlying mechanism `update()` uses to bound the
    // terminal band's shape-index range: paint the band into the SAME
    // `LayerId::background()` layer chrome already uses (no dedicated
    // layer), remember the shape count before ("`band_shape_start`"), and
    // read `all_entries().skip(start)` to identify exactly the range
    // appended since. Production (as of #436.4a) captures `band_shape_end`
    // the same way and hands back `[band_shape_start, band_shape_end)` as a
    // range via `App::take_terminal_band_range`, rather than cloning the
    // shapes out here — but the boundary-counting primitive these tests
    // exercise is identical either way. A full `FreminalGui`/`PerWindowState`
    // cannot be constructed headlessly (`freminal_windowing::WindowId` has
    // no public constructor outside the real winit event loop), so this
    // validates the extraction primitive directly against a bare
    // `egui::Context`, independent of the app.

    #[test]
    fn band_shape_range_extraction_finds_only_shapes_painted_after_start() {
        let ctx = egui::Context::default();

        // A shape painted *before* `band_shape_start` is captured (mirrors
        // chrome — menu bar, tab bar — painting into the background layer
        // earlier in the same pass) must NOT be included in the extracted
        // range.
        let mut extracted: Vec<egui::epaint::ClippedShape> = Vec::new();
        let mut chrome_shape_count = 0usize;
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let discarded_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            // "Chrome" shape, painted before the band region starts.
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(5.0, 5.0)),
                0.0,
                egui::Color32::BLUE,
            );
            chrome_shape_count = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            // Capture point, exactly as production does immediately before
            // the band region.
            let band_shape_start = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            // Band shapes, painted into the same `ui` (background layer).
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(10.0, 10.0)),
                0.0,
                egui::Color32::RED,
            );
            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(30.0, 30.0), egui::vec2(10.0, 10.0)),
                0.0,
                egui::Color32::GREEN,
            );

            extracted = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or_else(Vec::new, |list| {
                        list.all_entries().skip(band_shape_start).cloned().collect()
                    })
            });
        });
        discarded_output.drop_without_applying_deltas();

        assert_eq!(
            chrome_shape_count, 1,
            "sanity: exactly one chrome shape painted before the band"
        );
        assert_eq!(
            extracted.len(),
            2,
            "expected exactly the two band shapes, none of the chrome shape painted \
             before `band_shape_start`"
        );
    }

    #[test]
    fn band_shape_range_extraction_is_a_clone_not_a_drain() {
        // 436.2a's correctness requirement: extraction must NOT remove the
        // shapes from the background layer (that is deferred to 436.4,
        // alongside separate band painting). Confirms the real shapes are
        // still present after our clone-only extraction, and that egui's
        // own `end_pass` still drains them into `FullOutput.shapes` — i.e.
        // rendering is unaffected by the extraction seam existing.
        let ctx = egui::Context::default();

        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let band_shape_start = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or(0, |list| list.all_entries().len())
            });

            ui.painter().rect_filled(
                egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0)),
                0.0,
                egui::Color32::RED,
            );

            // Clone-only extraction (no removal from the layer), exactly as
            // production does in this subtask.
            let extracted: Vec<egui::epaint::ClippedShape> = ui.ctx().graphics(|g| {
                g.get(egui::LayerId::background())
                    .map_or_else(Vec::new, |list| {
                        list.all_entries().skip(band_shape_start).cloned().collect()
                    })
            });
            assert!(!extracted.is_empty());
        });
        full_output.textures_delta.clear();

        assert!(
            !full_output.shapes.is_empty(),
            "the background layer's shapes must still drain into FullOutput.shapes \
             (byte-identical rendering) because this subtask does not remove them \
             — that is 436.4's job, done together with separate band painting"
        );
    }

    // ── Regression test: same-layer widgets are not cross-layer-hidden ──
    //
    // This is the blocker the adversarial review caught in the FIRST attempt
    // at 436.2a: routing the terminal band into a *second*
    // `Order::Background` layer (distinct from `LayerId::background()`,
    // which chrome — menu bar, tab bar, and the `CentralPanel`'s own root
    // widget rect — paints into) trips egui 0.35's cross-layer hit-test
    // "hidden" rule (`egui-0.35.0/src/hit_test.rs:145-148`): a widget is
    // hidden from hover/click/drag if a LATER widget on a DIFFERENT layer
    // contains its rect. `CentralPanel`'s content-area widget rect covers
    // the whole band, so once the band moved to its own layer, every
    // `ui.interact()` widget inside the band (e.g. the command-block gutter
    // hover highlight) was liable to be permanently hidden — the two
    // untracked `Order::Background` layers tie-break by `IdMap` (hash)
    // iteration order (`nohash_hasher::IntMap`), which the second test below
    // reproduces deterministically for the exact `band_layer_id` scheme the
    // first attempt used, and is NOT controlled by paint call order.
    //
    // This is a real behavioral test (not merely structural): it drives two
    // full `egui::Context::run_ui` passes with an injected
    // `Event::PointerMoved`, exactly as a real frame would, and reads
    // `Response::hovered()` — which is the same mechanism the failing
    // command-block gutter hover highlight relies on.
    #[test]
    fn same_layer_widget_is_not_hidden_by_containing_widget() {
        let big_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        let small_rect = egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(20.0, 20.0));
        let pointer_pos = small_rect.center();

        let ctx = egui::Context::default();

        // Frame 1: register both widgets on the SAME layer as `ui`
        // (`LayerId::background()`) — mirroring the fixed `update()`, where
        // the band paints directly into `ui` rather than a dedicated
        // `band_layer_id`. `big` mimics `CentralPanel`'s content-area
        // widget rect (which fully contains the band); `small` mimics a
        // band-region interactive widget (e.g. the command-block gutter).
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let discarded_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let _small = ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
        });
        discarded_output.drop_without_applying_deltas();

        // Frame 2: pointer is over `small_rect`. Hit-testing (computed at
        // the start of this frame from frame 1's registered widget rects)
        // must find `band_widget` hovered — same-layer widgets are never
        // subject to the cross-layer "hidden" rule, regardless of paint
        // order, since `hit_test.rs` only hides a widget when
        // `current.layer_id != next.layer_id`.
        let raw_input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(pointer_pos)],
            ..Default::default()
        };
        let mut small_hovered = false;
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let discarded_output = ctx.run_ui(raw_input, |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let small_response = ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
            small_hovered = small_response.hovered();
        });
        discarded_output.drop_without_applying_deltas();

        assert!(
            small_hovered,
            "a widget fully contained by another widget on the SAME layer must \
             still be hoverable — this is the invariant the terminal band relies \
             on by staying in `LayerId::background()` rather than a dedicated layer"
        );
    }

    #[test]
    fn dedicated_background_layer_hides_contained_widget_cross_layer() {
        // Reproduces the actual blocker: the FIRST 436.2a attempt routed the
        // band into `band_layer_id` (a second `Order::Background` layer,
        // keyed by window id, exactly as constructed below) instead of
        // `LayerId::background()`. This deterministically demonstrates that
        // scheme hides a band widget behind the root content-area widget,
        // for this pinned egui/ahash version (the `IdMap` hash tie-break
        // ordering between two `Order::Background` layers is fixed for a
        // given ahash seed/version but is an internal implementation detail
        // — NOT something application code controls or should rely on,
        // which is precisely why the band must not use a second layer at
        // all, in either tie-break order).
        let big_rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 200.0));
        let small_rect = egui::Rect::from_min_size(egui::pos2(50.0, 50.0), egui::vec2(20.0, 20.0));
        let pointer_pos = small_rect.center();
        let band_layer_id = egui::LayerId::new(
            egui::Order::Background,
            egui::Id::new("freminal_terminal_band").with(egui::Id::new("probe_window")),
        );

        let ctx = egui::Context::default();
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let discarded_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let band_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(big_rect)
                    .layer_id(band_layer_id),
            );
            let _small = band_ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
        });
        discarded_output.drop_without_applying_deltas();

        let raw_input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(pointer_pos)],
            ..Default::default()
        };
        let mut small_hovered = false;
        // No painter here, so the egui 0.36 `TexturesDelta` drop-bomb (#8356)
        // must be defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let discarded_output = ctx.run_ui(raw_input, |ui| {
            let _big = ui.interact(
                big_rect,
                egui::Id::new("root_content_area"),
                egui::Sense::hover(),
            );
            let band_ui = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(big_rect)
                    .layer_id(band_layer_id),
            );
            let small_response = band_ui.interact(
                small_rect,
                egui::Id::new("band_widget"),
                egui::Sense::click(),
            );
            small_hovered = small_response.hovered();
        });
        discarded_output.drop_without_applying_deltas();

        assert!(
            !small_hovered,
            "expected the dedicated-layer scheme to reproduce the cross-layer \
             hidden-widget blocker (this pins down WHY that approach was reverted)"
        );
    }
}
