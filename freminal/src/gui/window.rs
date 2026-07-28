// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use egui;
use freminal_windowing::{RepaintProxy, WindowId};

use super::{
    PaneBorderDrag,
    chrome_damage::{ChromeSignals, ChromeTabSnapshot, DismissiblePresence},
    renderer::WindowPostRenderer,
    tabs::TabId,
    tabs::TabManager,
    terminal::FreminalTerminalWidget,
};

/// Pending window geometry from layout engine: `(size_px, position_px)`.
///
/// Each component is independent — either or both may be `Some`.
/// Position is typically `None` on Wayland.
pub(super) type PendingGeometry = (Option<[u32; 2]>, Option<[i32; 2]>);

/// Transient state for the resize overlay (issue #433): a passive,
/// window-centered HUD showing the terminal's new size while the user is
/// resizing the window or a split, fading out shortly after resizing stops.
#[derive(Debug, Clone, Copy)]
pub(super) struct ResizeOverlayState {
    /// The size to display, in character cells: (cols, rows).
    pub(super) size: (usize, usize),
    /// When the most recent resize event was observed. Drives the linger /
    /// fade-out timeout.
    pub(super) last_update: std::time::Instant,
}

/// How long the resize overlay lingers after the last genuine resize event
/// before disappearing entirely (issue #433).
pub(super) const RESIZE_OVERLAY_LINGER: std::time::Duration = std::time::Duration::from_millis(900);

/// How long, at the tail end of [`RESIZE_OVERLAY_LINGER`], the overlay fades
/// out linearly (issue #433).
pub(super) const RESIZE_OVERLAY_FADE: std::time::Duration = std::time::Duration::from_millis(250);

/// Alpha multiplier (1.0 = fully opaque, 0.0 = invisible) for the resize
/// overlay, given how long it has been since the last genuine resize event.
///
/// Fully opaque until the last `fade` window before `linger` elapses, then
/// fades linearly to 0, and is 0 at/after `linger`. Pure function so the
/// timing math is unit-testable without an egui frame (issue #433).
pub(super) fn resize_overlay_alpha(
    elapsed: std::time::Duration,
    linger: std::time::Duration,
    fade: std::time::Duration,
) -> f32 {
    if elapsed >= linger {
        return 0.0;
    }
    let remaining = linger.saturating_sub(elapsed);
    if remaining >= fade || fade.is_zero() {
        1.0
    } else {
        (remaining.as_secs_f32() / fade.as_secs_f32()).clamp(0.0, 1.0)
    }
}

/// Whether an observed char-grid size change is a GENUINE OS-window resize,
/// rather than the spurious `last_sent_size` churn caused by new
/// window/tab/split/pane-close/zoom transitions (which reset `last_sent_size`
/// to `(0, 0)`) (issue #433).
///
/// `window_resized` must be true only for a real resize between two KNOWN
/// window sizes (see `window_genuinely_resized` at the call site) — NOT for
/// the first `None -> Some` geometry observation of a freshly created window,
/// which would otherwise flash the overlay on launch / new-window spawn.
///
/// Split-border drags are deliberately excluded: the overlay reports the
/// whole-window `cols × rows`, which does not change when an internal border
/// moves, so showing a frozen number during a split drag would be
/// uninformative.
pub(super) const fn resize_is_genuine(size_changed: bool, window_resized: bool) -> bool {
    size_changed && window_resized
}

/// Per-window state for a single OS window.
///
/// Each window (whether it was the first or spawned later via `Ctrl+Shift+N`)
/// owns one of these. All windows are peers — there is no root/secondary
/// distinction. Shared state (config, args, binding map, settings modal)
/// lives on [`super::FreminalGui`].
// Each bool is an independent, short-lived per-frame UI intent flag
// (pending close-pane / new-window / force-close, etc.) drained at the end of
// the frame. They are unrelated and combining them into a state machine would
// couple distinct intents and obscure meaning -- same rationale as the
// `FreminalGui` aggregator's allow.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct PerWindowState {
    /// All open terminal tabs for this window.
    pub(super) tabs: TabManager,

    /// Terminal widget: owns font manager, shaping cache, glyph atlas metadata.
    /// Created lazily on the first frame when the egui context is available.
    pub(super) terminal_widget: FreminalTerminalWidget,

    /// Last title string sent to the OS window via `ViewportCommand::Title`.
    pub(super) last_window_title: String,

    /// Cached OS dark/light preference for this window.
    pub(super) os_dark_mode: bool,

    /// Cached egui style inputs — prevents redundant `global_style_mut` calls.
    ///
    /// Key tuple: `(&'static ThemePalette, background_opacity, GuiTheme)`.  A
    /// change in any element invalidates the cache and forces a full
    /// `build_visuals` rebuild.  `GuiTheme` is compared by value (it is
    /// `PartialEq` but not `Eq` because of its `f32` fields), so the comparison
    /// is done manually in `app_impl::update`.
    pub(super) style_cache: Option<(
        &'static freminal_common::themes::ThemePalette,
        f32,
        freminal_common::gui_theme::GuiTheme,
    )>,

    /// Set to `true` by the `ClosePane` key action dispatch; consumed at the
    /// end of the frame.
    pub(super) pending_close_pane: bool,

    /// Pending directional focus change; consumed at the end of the frame.
    pub(super) pending_focus_direction: Option<freminal_common::keybindings::KeyAction>,

    /// Active pane border drag state (mouse drag-to-resize).
    pub(super) border_drag: Option<PaneBorderDrag>,

    /// Active resize-overlay HUD state, or `None` when no resize is in
    /// progress / the overlay has timed out (issue #433).
    pub(super) resize_overlay: Option<ResizeOverlayState>,

    /// Last modified time of the shader file, used for hot-reload detection.
    /// `None` when no shader is configured or hot-reload is disabled.
    pub(super) shader_last_mtime: Option<std::time::SystemTime>,

    /// Per-window post-processing renderer (FBO + custom shader).
    ///
    /// Each window owns its own `WindowPostRenderer` so that pane
    /// `PaintCallback`s write into this window's FBO — not another window's.
    ///
    /// The `Arc<Mutex<…>>` wrapper is for GUI-thread interior mutability
    /// inside `PaintCallback` captures, not cross-thread synchronisation —
    /// this is only accessed on the GUI thread. See [`RenderState`]
    /// (in `gui::terminal::widget`) for the full rationale.
    pub(super) window_post: Arc<Mutex<WindowPostRenderer>>,

    /// Per-window GL renderer state for the toast overlay (issue #433).
    /// `Arc<Mutex<…>>` is GUI-thread interior mutability for the
    /// `Send + Sync + 'static` `PaintCallback` capture (mirrors `window_post`),
    /// not cross-thread sync.
    pub(super) toast_render_state:
        std::sync::Arc<std::sync::Mutex<crate::gui::renderer::ToastRenderState>>,

    /// Shared repaint handle for this window's PTY threads.
    ///
    /// Each window gets its own `Arc<OnceLock<(RepaintProxy, WindowId)>>`
    /// so PTY threads repaint the correct window.
    pub(super) repaint_handle: Arc<OnceLock<(RepaintProxy, WindowId)>>,

    /// Set to `true` by the `NewWindow` key action or menu; consumed in
    /// `update()` where `WindowHandle` is available.
    pub(super) pending_new_window: bool,

    /// If set, send resize + reposition viewport commands on the next frame.
    ///
    /// Populated by the layout engine when applying a layout to an existing
    /// window.  Consumed in `update()` via `ctx.send_viewport_cmd`.
    /// Each component is independent — either or both may be `Some`.
    pub(super) pending_geometry: Option<PendingGeometry>,

    /// Last known inner size (width, height) in physical pixels.
    ///
    /// Updated every frame from `ctx.input(|i| i.screen_rect())`.  Used by
    /// `save_layout` to persist window geometry without needing `ctx`.
    pub(super) last_known_size: Option<[u32; 2]>,

    /// Last known outer position in physical pixels.
    ///
    /// Updated every frame from `ViewportInfo::outer_rect` when available.
    /// `None` on Wayland (position is not reported) or before the first frame.
    pub(super) last_known_position: Option<[i32; 2]>,

    /// Tab currently being renamed via an inline text editor.
    ///
    /// Set by `KeyAction::RenameTab` (renames the active tab) or a
    /// double-click on a tab label.  While `Some`, the tab bar renders
    /// a `TextEdit` widget in place of the label for this tab.
    ///
    /// Cleared when the user commits (Enter) or cancels (Escape) the
    /// rename, or when the target tab is closed.
    pub(super) renaming_tab: Option<TabId>,

    /// Scratch buffer for the in-progress rename.
    ///
    /// Initialised from the target tab's current display name when
    /// `renaming_tab` is set, mutated by the `TextEdit`, and consumed on
    /// commit.  Cleared when rename ends.
    pub(super) rename_buffer: String,

    /// Index of the tab currently being dragged, if any.
    ///
    /// Set when a mouse drag starts on a tab label, cleared when the drag
    /// ends (at which point a `TabBarAction::Reorder` is emitted if the
    /// pointer was released over a different tab).
    pub(super) dragging_tab: Option<usize>,

    /// Tab rects indexed by original tab position, used to compute the
    /// drop slot during a drag.
    ///
    /// Captured at the end of each frame's `show_tab_bar`, but **frozen**
    /// for the duration of a drag: once `dragging_tab` is set, rects are
    /// NOT refreshed until the drag ends. Freezing is essential because
    /// the rendered (preview) rects shift as the ghost moves between
    /// slots — if we used those shifted rects to decide the next frame's
    /// slot, differently-sized tabs would oscillate between orderings
    /// (pointer crosses center → swap → new rect moves under pointer →
    /// swap back). Freezing rects to the natural pre-drag layout gives
    /// stable decision boundaries for the entire drag.
    pub(super) last_tab_rects: Vec<egui::Rect>,

    /// `KeyAction`s triggered from menu items (Edit, Help, etc.) that must
    /// run against the active pane's `ViewState` + PTY `input_tx`.
    ///
    /// The menu bar itself cannot dispatch these directly because it runs
    /// with `&mut PerWindowState` (no pane view-state access), so it pushes
    /// here.  Drained and dispatched at the top of the active pane's input
    /// processing each frame.
    pub(super) pending_menu_actions: Vec<freminal_common::keybindings::KeyAction>,

    /// Smart paste guard confirmation dialog for this window (Task 77).
    ///
    /// Opened by `guarded_paste` when the analyzer flags a payload, rendered
    /// every frame while open, and resolved when the user confirms or cancels.
    pub(super) paste_dialog: super::paste_guard::PasteDialog,

    /// Broadcast-input confirmation dialog for this window (Task 74).
    ///
    /// Opened by the `ToggleBroadcastInput` dispatch when
    /// `[tabs] confirm_broadcast` is set and broadcast is being turned on.
    pub(super) broadcast_dialog: super::broadcast_guard::BroadcastConfirmDialog,

    /// Close-on-running-command confirmation dialog for this window (Task 98).
    ///
    /// Opened by a guarded pane / tab / window close when the affected scope
    /// contains a running foreground command, rendered every frame while open,
    /// and resolved to Cancel or Force Close.
    pub(super) close_dialog: super::close_guard::CloseGuardDialog,

    /// Set by the `ForceClose` key action; consumed in `update()` where the
    /// close dialog is resolved.  Resolves an open close-guard dialog as
    /// "Force Close" without the user reaching for the mouse or Ctrl+Enter.
    pub(super) pending_force_close: bool,

    /// Raw key events for the egui-blocked key set (Task 114.5/114.7:
    /// keypad operators/directional, media, print/pause/menu keys), queued
    /// by `App::on_raw_key_event` at winit-event time.
    ///
    /// Encoding cannot happen inside `on_raw_key_event` itself — that
    /// callback fires outside the render/`update()` path, where the active
    /// pane, its snapshot, and the true per-pane `super_pressed` state are
    /// not in scope (and `super_pressed` is only updated during render, so
    /// encoding at event time risks a stale-super hazard for chorded keys).
    /// Instead, events are pushed here and drained once per frame on the
    /// render path — mirroring the `pending_menu_actions` /
    /// `pending_close_pane` deferred-queue precedent on this struct — at the
    /// point where the active pane's fresh `super_pressed` state is
    /// available.
    pub(super) pending_raw_keys: Vec<(
        freminal_windowing::RawKeyEvent,
        freminal_windowing::RawKeyMods,
    )>,

    /// Frame-damage report for the most recent `update()` of this window
    /// (#435), drained by `App::take_frame_damage`.
    ///
    /// Set at the end of each `update()`: [`FrameDamage::Partial`] with the
    /// cursor damage rect(s) when the frame was a pure cursor-only update
    /// (every rendered pane took the cursor-only fast path and nothing else in
    /// the window changed), otherwise [`FrameDamage::Full`]. Defaults to
    /// `Full` so a window that has not yet rendered, or any frame the
    /// aggregation does not positively prove cursor-only, presents fully.
    ///
    /// [`FrameDamage::Partial`]: freminal_windowing::FrameDamage::Partial
    /// [`FrameDamage::Full`]: freminal_windowing::FrameDamage::Full
    pub(super) pending_frame_damage: freminal_windowing::FrameDamage,

    /// Shape-index range for the "terminal band" (the pre-clear FBO
    /// callback, the per-pane render loop, the post-shader composite
    /// callback, pane border lines, and the broadcast label) within this
    /// frame's `full_output.shapes`, drained by
    /// `App::take_terminal_band_range` (#436.4a; supersedes the #436.2a
    /// shape-cloning approach previously exposed via the now-removed
    /// `take_terminal_band_shapes`).
    ///
    /// Set at the end of each `update()` to `band_shape_start..band_shape_end`
    /// — the range appended to `LayerId::background()`'s `PaintList` since
    /// `band_shape_start` was captured — see the extraction comment at the
    /// `band_shape_end` binding in `update()`. The band paints
    /// into the SAME background layer chrome uses (not a dedicated layer:
    /// routing it into a second `Order::Background` layer trips egui's
    /// cross-layer hit-test "hidden" rule and suppresses band widget
    /// interaction — see the capture-point comments in `update()`). Since
    /// the background layer drains first into `FullOutput.shapes`, this
    /// range is valid as-is against `full_output.shapes` in `run_frame`.
    /// Defaults to `0..0` before the first frame.
    pub(super) pending_terminal_band_range: std::ops::Range<usize>,

    /// The `(active tab, active pane)` shown on the previous frame.
    ///
    /// Compared each frame to detect when the active pane changes — whether by
    /// a pane switch within a tab or by a tab switch (which changes the active
    /// pane too). On a change, the newly-active pane's cursor blink phase is
    /// re-anchored so its cursor appears immediately rather than inheriting the
    /// global blink cycle's current half. `None` before the first frame.
    pub(super) previous_active_pane_key: Option<(TabId, crate::gui::panes::PaneId)>,

    /// Authoritative partial-present flag for this window (#435).
    ///
    /// The windowing layer stores into this each frame — `true` when it
    /// skipped the full clear and is presenting only the damage region,
    /// `false` for a normal full clear + present — **before** the pane paint
    /// callbacks run. The callbacks read it (a clone is captured into each)
    /// to gate their scissor optimization, so a pane only scissors its redraw
    /// when the clear was actually skipped. Shared via `Arc` because the pane
    /// `PaintCallback` closures require `'static` captures; only ever touched
    /// on the GUI thread, so `Relaxed` ordering suffices.
    pub(super) present_is_partial: std::sync::Arc<std::sync::atomic::AtomicBool>,

    /// Chrome-damage decision for the most recent `update()` of this window
    /// (#436.3), drained by `App::take_chrome_damage`.
    ///
    /// Set at the end of each `update()` from [`super::chrome_damage::decide_chrome_damage`].
    /// Defaults to [`freminal_windowing::ChromeDamage::Changed`] — the
    /// conservative, always-correct behavior for a window that has not yet
    /// rendered, or any frame whose computation is skipped by an early
    /// return (mirrors `pending_frame_damage`'s same risk/precedent).
    pub(super) pending_chrome_damage: freminal_windowing::ChromeDamage,

    /// The individual #436 §3.3 signals computed during the most recent
    /// `update()` of this window, staged here because most of them are only
    /// available inside the `CentralPanel` closure while the final decision
    /// (which also needs the post-toast-render dismissible-presence sample)
    /// can only be made after that closure returns. Combined with the §3.5
    /// presence-transition/settle inputs into `pending_chrome_damage` right
    /// before `update()` returns. Defaults to all-`false` — harmless, since
    /// it is always overwritten before being read on any frame that reaches
    /// the point where `pending_chrome_damage` is computed.
    pub(super) pending_chrome_signals: ChromeSignals,

    /// #436 §3.5 self-dismissal settle rule: `true` when a dismissible
    /// element (toast, About, Welcome, paste/broadcast/close-guard dialogs,
    /// save-layout prompt) transitioned presence on the PREVIOUS frame, which
    /// forces THIS frame `ChromeDamage::Changed` too (the "settle frame").
    /// Reassigned every frame to that frame's own transition result — see
    /// `chrome_damage::decide_chrome_damage`'s doc for why this needs no
    /// separate reset step. `false` before the first frame.
    pub(super) chrome_settle_pending: bool,

    /// Presence of every dismissible chrome element, sampled once at the end
    /// of the previous frame (after all `.show()` calls that frame,
    /// including the toast stack's).
    ///
    /// Compared against this frame's own after-`.show()` sample to catch a
    /// transition NOT caused by that element's own self-dismissal (e.g. a
    /// menu action closing a dialog) — the cross-frame half of the §3.5
    /// settle rule. The intra-frame (before-vs-after within a single frame)
    /// half, which is what catches the toast self-dismissal hazard
    /// (adversarial finding 1), uses a frame-local `before`/`after` pair
    /// instead and does not need to be stored here. Defaults to
    /// all-`false` (nothing dismissible present before the first frame).
    pub(super) prev_dismissible_presence: DismissiblePresence,

    /// Previous frame's tab/pane snapshot for the §3.3 tab-set / tab-title /
    /// pane-layout / broadcast-state change-detection rows (#436.3). See
    /// [`super::chrome_damage::ChromeTabSnapshot`] and
    /// [`super::chrome_damage::diff_tab_snapshots`]. Defaults to empty,
    /// which naturally reports every row as "changed" on the first
    /// comparison — harmless, since the first few frames are also covered
    /// by the warm-up counter below.
    pub(super) prev_chrome_tab_snapshot: ChromeTabSnapshot,

    /// Previous frame's `window_focused` value (#436.3 §3.3 "Window focus
    /// change" row). Compared each frame to the freshly-read value to
    /// detect focus in/out. `false` before the first frame.
    pub(super) prev_window_focused: bool,

    /// Frames rendered since this window was created, saturating at
    /// [`super::chrome_damage::WARMUP_FRAMES`] (#436.3 §7 warm-up). While
    /// below that count, `ChromeSignals::warming_up` is `true`,
    /// unconditionally forcing `ChromeDamage::Changed`.
    pub(super) chrome_frames_rendered: u32,

    /// The delay `update()` itself requested via `ctx.request_repaint_after`
    /// on the most recent frame (#436.4b §3.1 amendment), drained by
    /// `App::take_terminal_requested_delay`.
    ///
    /// Set at the end of each `update()` from `shortest_repaint_delay` (the
    /// shortest interval any rendered pane needed — cursor blink, content
    /// update, or shader animation). Compared against egui's own requested
    /// repaint delay by `egui_integration::chrome_repaint_settled` to decide
    /// whether a REPLAY is permitted: a REPLAY requires that nothing OTHER
    /// than this frame's own request also wants a wake. Defaults to `None`.
    pub(super) pending_terminal_requested_delay: Option<std::time::Duration>,

    /// The `CentralPanel` content rect (`ui.available_rect_before_wrap()`)
    /// captured on the most recent FULL frame (#436.4b).
    ///
    /// On a REPLAY frame `update()` skips building the menu bar, tab bar,
    /// and `CentralPanel` (all cached chrome), so there is no fresh
    /// `available_rect` to read the terminal band's content rect from.
    /// Instead the band's `Ui` is constructed directly at this cached rect,
    /// in the same background layer chrome uses — valid because a REPLAY is
    /// only permitted when chrome (including window size) is proven
    /// unchanged since the frame that last set this field. `None` before
    /// the first FULL frame (a REPLAY can never be chosen then, since
    /// `chrome_cache` is also `None` at that point).
    pub(super) cached_central_rect: Option<egui::Rect>,

    /// #436.8 menu-bar + tab-bar rects (egui logical points), captured on FULL
    /// frames (REPLAY skips building the panels). `None` until the first FULL
    /// frame => `is_chrome_interactive_at` returns the conservative `true`.
    pub(super) chrome_head_rects: Option<Vec<egui::Rect>>,
    /// #436.8 split-border drag-sensor rects (egui logical points), rebuilt every
    /// frame; explicitly cleared on frames that build no sensors (single pane /
    /// zoomed / overlay open).
    pub(super) chrome_border_rects: Vec<egui::Rect>,

    /// Per-frame render attribution counters (diagnostic), flushed to a
    /// `debug` log line every [`FrameStats::FLUSH_EVERY`] drawn frames. Lets
    /// a CPU investigation see, without a profiler, how many frames are
    /// actually drawn and how they split across the #435 per-frame damage
    /// classes (a genuinely-unchanged redraw costs no CPU rebuild; a `Full`
    /// frame rebuilds the visible vertex data).
    pub(super) frame_stats: FrameStats,
}

/// Diagnostic per-frame render attribution (see [`PerWindowState::frame_stats`]).
///
/// Every counted frame is one `update()` call for a window (i.e. one drawn
/// frame). `unchanged`/`cursor_only`/`full` classify the ACTIVE pane's
/// `PaneFrameDamage` that frame (the #435 signal), and
/// `blink_wake_suppressed` counts frames where a blink-style cursor was
/// hidden (DECTCEM / inactive / echo-off) so the old ~500ms blink wake was
/// correctly NOT scheduled (validates the `cursor_blink_wants_repaint` fix —
/// each such frame is a ~2Hz phantom wake that no longer happens).
#[derive(Debug, Default)]
pub(super) struct FrameStats {
    pub(super) frames_drawn: u64,
    pub(super) unchanged: u64,
    pub(super) cursor_only: u64,
    pub(super) full: u64,
    pub(super) blink_wake_suppressed: u64,

    // ── Task 121 frame-profiling harness (feature-gated) ──────────────────
    //
    // Every field below is `#[cfg(feature = "frame-profiling")]`: a default
    // build must not carry so much as an extra counter increment in the
    // frame path, since timing calls there would perturb the very thing
    // being measured. See `agents.md` / the `freminal-bench-table` skill.
    /// Frames where `App::update` received `ChromeMode::Full`.
    #[cfg(feature = "frame-profiling")]
    pub(super) chrome_mode_full: u64,
    /// Frames where `App::update` received `ChromeMode::Replay`. Answers
    /// "does Replay ever actually engage in a live session, and at what
    /// duty cycle" — no counter for this existed anywhere before Task 121.
    #[cfg(feature = "frame-profiling")]
    pub(super) chrome_mode_replay: u64,
    /// Frames where every rendered pane reported `PaneFrameDamage::Unchanged`
    /// (and no bell/toast/force-full override applied) yet the frame was
    /// still presented. `decide_frame_damage` has no representation for "no
    /// pane changed anything" distinct from "some pane needs a full
    /// rebuild" -- both fall through to `FrameDamage::Full` when the damage
    /// rect list ends up empty -- so this counts how often that happens.
    #[cfg(feature = "frame-profiling")]
    pub(super) zero_change_presented: u64,
    /// Frames whose final `win.pending_frame_damage` was `FrameDamage::Full`.
    #[cfg(feature = "frame-profiling")]
    pub(super) frame_damage_full: u64,
    /// Frames whose final `win.pending_frame_damage` was
    /// `FrameDamage::Partial(_)`.
    #[cfg(feature = "frame-profiling")]
    pub(super) frame_damage_partial: u64,
    /// Cumulative wall-clock time spent in `central_body`'s own bookkeeping
    /// this session -- everything in the closure EXCEPT the per-pane
    /// `terminal_widget.show()` calls themselves (window-manipulation
    /// drain, OSC 9/52/99 routing, border drag-sensor rebuild, the
    /// resize-debounce/scroll-sync bookkeeping interleaved in the per-pane
    /// loop, and the post-loop focus-follows-mouse hit-testing / title /
    /// repaint-scheduling tail). Computed as `central_body`'s total elapsed
    /// time minus `phase_panes_total`'s contribution for that same frame,
    /// rather than instrumenting every individual sub-block, since the
    /// bookkeeping is interleaved with (not cleanly separable from) the
    /// per-pane loop -- see the comment at the `central_body` call site.
    #[cfg(feature = "frame-profiling")]
    pub(super) phase_orchestration_total: std::time::Duration,
    /// The single largest per-frame `phase_orchestration` contribution observed
    /// this session (a mean hides the tail).
    #[cfg(feature = "frame-profiling")]
    pub(super) phase_orchestration_max: std::time::Duration,
    /// Cumulative wall-clock time spent inside the per-pane
    /// `terminal_widget.show()` calls this session (summed across every
    /// pane rendered every frame).
    #[cfg(feature = "frame-profiling")]
    pub(super) phase_panes_total: std::time::Duration,
    /// The single largest per-frame `phase_panes` contribution (summed
    /// across that frame's panes) observed this session.
    #[cfg(feature = "frame-profiling")]
    pub(super) phase_panes_max: std::time::Duration,
    /// Cumulative wall-clock time spent inside the whole productive body of
    /// `App::update` this session (Task 121 defect-1 fix): captured from an
    /// `update_start = Instant::now()` at the very top of `update`, through
    /// `.elapsed()` taken after `compose_with_chrome_damage` -- i.e. AFTER
    /// `central_body` returns, so it also covers the menu/tab bar
    /// construction, dead-pane cleanup / snapshot bootstrap /
    /// `poll_session_autosave` / settings-window dispatch, the toast stack's
    /// `.show()`, and the chrome-damage "after" sample + decision +
    /// composition -- all of which used to be misattributed to "egui
    /// overhead" because the old `central_body_start` timer only started
    /// once the `central_body` closure was invoked.
    ///
    /// With this field, the analyst can compute freminal's own total cost as
    /// `phase_app_update`; freminal's chrome-construction cost as
    /// `phase_app_update` minus `phase_orchestration` minus `phase_panes`;
    /// and egui's own overhead as the windowing crate's own `run_ui_total`
    /// minus this field, plus its `tessellate_total` and `paint_total`
    /// (since `run_ui` wraps the `App::update` call this field measures).
    #[cfg(feature = "frame-profiling")]
    pub(super) phase_app_update_total: std::time::Duration,
    /// The single largest per-frame `phase_app_update` contribution observed
    /// this session (a mean hides the tail).
    #[cfg(feature = "frame-profiling")]
    pub(super) phase_app_update_max: std::time::Duration,
}

impl FrameStats {
    /// Emit a `debug` summary once every this many drawn frames.
    pub(super) const FLUSH_EVERY: u64 = 120;
}

#[cfg(feature = "frame-profiling")]
impl FrameStats {
    /// Percentage of `(chrome_mode_full + chrome_mode_replay)` frames that
    /// were `Replay`. Pure so it's unit-testable without constructing a
    /// window or an egui frame. `0.0` when no frames have been counted yet
    /// (rather than dividing by zero).
    ///
    /// Deliberately duplicated in `freminal_windowing::egui_integration::FrameProfile`
    /// rather than shared (reviewed and accepted) -- keep the two in sync by
    /// eye if either changes.
    pub(super) fn chrome_replay_duty_cycle_pct(full: u64, replay: u64) -> f64 {
        let total = full.saturating_add(replay);
        if total == 0 {
            return 0.0;
        }
        // `u64 -> f64` is lossy for very large counts (beyond 2^53), but a
        // live session's frame counters never approach that range; `approx_as`
        // is the established lossy-conversion idiom in this codebase (see
        // e.g. `egui_integration.rs`'s `scale_factor().approx_as::<f32>()`).
        let replay_f: f64 = conv2::ConvUtil::approx_as(replay).unwrap_or(0.0);
        let total_f: f64 = conv2::ConvUtil::approx_as(total).unwrap_or(1.0);
        (replay_f / total_f) * 100.0
    }

    /// Mean of a cumulative `Duration` sum over `count` samples, as a
    /// `Duration`. Returns `Duration::ZERO` for `count == 0` rather than
    /// dividing by zero. Pure so it's unit-testable in isolation.
    ///
    /// Deliberately duplicated in `freminal_windowing::egui_integration::FrameProfile`
    /// rather than shared (reviewed and accepted) -- keep the two in sync by
    /// eye if either changes.
    pub(super) fn mean_duration(total: std::time::Duration, count: u64) -> std::time::Duration {
        if count == 0 {
            return std::time::Duration::ZERO;
        }
        let count_f: f64 = conv2::ConvUtil::approx_as(count).unwrap_or(1.0);
        total.div_f64(count_f.max(1.0))
    }
}

#[cfg(all(test, feature = "frame-profiling"))]
mod frame_profiling_tests {
    use super::FrameStats;
    use std::time::Duration;

    #[test]
    fn duty_cycle_is_zero_with_no_frames() {
        assert!((FrameStats::chrome_replay_duty_cycle_pct(0, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duty_cycle_is_zero_when_replay_never_engages() {
        assert!((FrameStats::chrome_replay_duty_cycle_pct(120, 0) - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn duty_cycle_is_100_when_always_replay() {
        let pct = FrameStats::chrome_replay_duty_cycle_pct(0, 120);
        assert!((pct - 100.0).abs() < 0.001, "pct was {pct}");
    }

    #[test]
    fn duty_cycle_is_75_for_1_full_3_replay() {
        let pct = FrameStats::chrome_replay_duty_cycle_pct(30, 90);
        assert!((pct - 75.0).abs() < 0.001, "pct was {pct}");
    }

    #[test]
    fn mean_duration_is_zero_with_no_samples() {
        assert_eq!(
            FrameStats::mean_duration(Duration::from_millis(500), 0),
            Duration::ZERO
        );
    }

    #[test]
    fn mean_duration_divides_evenly() {
        let mean = FrameStats::mean_duration(Duration::from_millis(1200), 120);
        assert_eq!(mean, Duration::from_millis(10));
    }

    #[test]
    fn mean_duration_handles_a_single_sample() {
        let mean = FrameStats::mean_duration(Duration::from_micros(250), 1);
        assert_eq!(mean, Duration::from_micros(250));
    }
}

#[cfg(test)]
mod resize_overlay_tests {
    use super::{
        RESIZE_OVERLAY_FADE, RESIZE_OVERLAY_LINGER, resize_is_genuine, resize_overlay_alpha,
    };
    use std::time::Duration;

    #[test]
    fn alpha_is_full_at_zero_elapsed() {
        assert!(
            (resize_overlay_alpha(Duration::ZERO, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE)
                - 1.0)
                .abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn alpha_is_full_just_before_fade_window_starts() {
        // linger=900ms, fade=250ms -> fade starts at elapsed=650ms.
        let elapsed = RESIZE_OVERLAY_LINGER
            .saturating_sub(RESIZE_OVERLAY_FADE)
            .checked_sub(Duration::from_millis(1))
            .unwrap_or(Duration::ZERO);
        let alpha = resize_overlay_alpha(elapsed, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE);
        assert!((alpha - 1.0).abs() < 0.01, "alpha was {alpha}");
    }

    #[test]
    fn alpha_is_half_midway_through_fade() {
        // elapsed = linger - fade/2 -> remaining = fade/2 -> alpha = 0.5.
        let elapsed = RESIZE_OVERLAY_LINGER
            .checked_sub(RESIZE_OVERLAY_FADE / 2)
            .unwrap_or(Duration::ZERO);
        let alpha = resize_overlay_alpha(elapsed, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE);
        assert!((alpha - 0.5).abs() < 0.01, "alpha was {alpha}");
    }

    #[test]
    fn alpha_is_zero_at_and_after_linger() {
        let at_linger = resize_overlay_alpha(
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE,
        );
        assert!(at_linger.abs() < f32::EPSILON, "alpha was {at_linger}");
        let past_linger = resize_overlay_alpha(
            RESIZE_OVERLAY_LINGER + Duration::from_secs(5),
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE,
        );
        assert!(past_linger.abs() < f32::EPSILON, "alpha was {past_linger}");
    }

    #[test]
    fn genuine_requires_size_change_and_window_resize() {
        // Truth table: size_changed, window_resized -> expected.
        assert!(!resize_is_genuine(false, false));
        assert!(!resize_is_genuine(false, true));
        // The spurious case the adversarial review flagged: char-grid size
        // changed (e.g. new tab/split reset last_sent_size to (0,0), or the
        // first `None -> Some` geometry observation on launch) but the OS
        // window did not genuinely resize between two known sizes.
        assert!(!resize_is_genuine(true, false));
        // The only genuine case: a real OS-window resize alongside a char-grid
        // change.
        assert!(resize_is_genuine(true, true));
    }
}
