// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::{Arc, Mutex, OnceLock};

use egui;
use freminal_windowing::{RepaintProxy, WindowId};

use super::{
    PaneBorderDrag,
    chrome_damage::{ChromeTabSnapshot, DismissiblePresence},
    published_frame_state::PublishedFrameState,
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

/// Whether the resize overlay is currently mid-fade (or past `linger`) at
/// `elapsed` since the last genuine resize event (subtask 121.14).
///
/// `false` only during the fully-opaque phase (`elapsed < linger - fade`),
/// where the overlay's painted pixels are not changing frame-to-frame, so a
/// caller may safely treat "no animation is in flight" as true for it —
/// this is the predicate `pointer_motion_needs_repaint` consults instead of
/// the old `resize_overlay.is_some()` presence test, which disabled
/// suppression for the overlay's ENTIRE 900ms life instead of just the
/// final 250ms fade.
///
/// **Returns `true` for `elapsed >= linger`, NOT `false`.** This is the
/// non-obvious part: the overlay is only ever cleared by a rendered frame
/// reaching the `clear_resize_overlay` call in `app_impl.rs`'s
/// resize-overlay block — nothing else clears it. If this
/// predicate went `false` again past `linger` (before that clearing frame
/// had actually run), a caller using it to gate "may I suppress waking this
/// window" would stop scheduling frames while the overlay was still
/// `Some`, and the HUD would be stranded on screen at whatever it was last
/// painted with — never reaching the frame that removes it. Treating "at or
/// past `linger`" as still-animating guarantees at least one more wake
/// reaches that clearing frame.
///
/// Pure function so the timing math is unit-testable without an egui frame,
/// mirroring [`resize_overlay_alpha`].
pub(super) fn resize_overlay_is_animating(
    elapsed: std::time::Duration,
    linger: std::time::Duration,
    fade: std::time::Duration,
) -> bool {
    elapsed >= linger.saturating_sub(fade)
}

/// The repaint delay the resize-overlay HUD should request at `elapsed`
/// since the last genuine resize event (subtask 121.14, Part A step 3).
///
/// While still fully opaque (`elapsed < linger - fade`), the overlay's
/// pixels are not changing, so requesting an unconditional 16ms cadence
/// every frame it is alive is wasted work — a wake timed to land exactly at
/// fade-start is sufficient (and gives an equally smooth start to the fade
/// once it begins). Once fading — or at/past `linger`, mirroring
/// [`resize_overlay_is_animating`]'s boundary and its "still needs a wake"
/// reasoning — request the fast 16ms cadence so the fade (or the final
/// frame that clears the overlay) proceeds smoothly.
///
/// **MUST stay consistent with [`resize_overlay_is_animating`]'s boundary.**
/// The two are read together at two different call sites (this decides what
/// the HUD itself asks for; that decides whether pointer-motion suppression
/// may engage) — if they disagree, the HUD either janks (suppression
/// engages while this still asks for 16ms, evidence of "still animating"
/// otherwise ignored) or never sleeps (this keeps asking for 16ms while
/// suppression has already been judged safe to allow).
///
/// Pure function so the schedule is unit-testable without an egui frame.
pub(super) fn resize_overlay_repaint_delay(
    elapsed: std::time::Duration,
    linger: std::time::Duration,
    fade: std::time::Duration,
) -> std::time::Duration {
    let fade_start = linger.saturating_sub(fade);
    if elapsed < fade_start {
        fade_start.saturating_sub(elapsed)
    } else {
        std::time::Duration::from_millis(16)
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

    /// Group A (#122.4): cached rects, staged chrome signals, and the
    /// resize-overlay HUD — everything written once per completing
    /// `App::update` and read only from `is_chrome_interactive_at` /
    /// `pointer_motion_needs_repaint`, outside any frame. See
    /// [`PublishedFrameState`] for the full publish/read discipline this
    /// replaces (formerly seven separate fields:
    /// `cached_central_rect`, `cached_gutter_inset_logical`,
    /// `chrome_head_rects`, `chrome_border_rects`, `chrome_toast_rects`,
    /// `pending_chrome_signals`, `resize_overlay`).
    pub(super) published: PublishedFrameState,

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
    /// Task 121 frame-profiling harness follow-up (issue #459/#461
    /// gate-blocker investigation): cumulative per-frame fired count for
    /// each of the 15 `chrome_damage::ChromeSignals` §3.3 fields, indexed
    /// the SAME as `ChromeSignals::named_fields()`'s array order (see that
    /// method's doc for the exhaustiveness guarantee this indexing relies
    /// on). Incremented in `app_impl.rs` right after
    /// `win.published`'s chrome signals are published for the frame.
    #[cfg(feature = "frame-profiling")]
    pub(super) chrome_signal_fired_counts: [u64; 15],

    // ── Task 121 pointer-motion repaint-gate condition counters (diagnostic;
    // feature-gated) ────────────────────────────────────────────────────
    //
    // `Cell<u64>`, not a plain `u64` like every other counter on this
    // struct: `App::pointer_motion_needs_repaint` takes `&self` (see the
    // `freminal_windowing::App` trait) and reaches `PerWindowState` (and
    // this `FrameStats`) via `self.windows.get(&window_id)` — an
    // IMMUTABLE borrow — so these counters must be mutable through a
    // shared reference. The GUI runs single-threaded on the winit/egui
    // event-loop thread, so `Cell` (not `AtomicU64`/`Mutex`) is the
    // correct, minimal tool; there is no concurrent access to race.
    //
    // Reset every `FLUSH_EVERY`-frame flush window (see
    // `reset_pointer_condition_window`), UNLIKE `chrome_signal_fired_counts`
    // above (which is cumulative since window creation): this diagnostic
    // answers "which repaint-gate condition(s) are firing on pointer motion
    // RIGHT NOW", which only makes sense windowed — the same reasoning as
    // `egui_integration.rs`'s settle-gate value diagnostics.
    /// Total `App::pointer_motion_needs_repaint` calls observed since the
    /// last flush-window reset — the denominator for reading each
    /// per-condition count below as a percentage.
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_repaint_check_total: std::cell::Cell<u64>,
    /// `chrome_interactive` fired count (see
    /// `PointerMotionConditionFlags::chrome_interactive`).
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_chrome_interactive: std::cell::Cell<u64>,
    /// `any_pane_selecting` fired count.
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_any_pane_selecting: std::cell::Cell<u64>,
    /// `overlay_open` fired count.
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_overlay_open: std::cell::Cell<u64>,
    /// `pointer_pane_unresolved` fired count.
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_pointer_pane_unresolved: std::cell::Cell<u64>,
    /// `mouse_tracking_active` fired count (pane-resolved calls only).
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_mouse_tracking_active: std::cell::Cell<u64>,
    /// `has_urls` fired count (pane-resolved calls only) — one of the three
    /// independent sub-terms of `pane_hover_region_risk`'s disjunction,
    /// counted separately from `scroll_offset_nonzero`/`gutter_active`
    /// rather than as one aggregate: distinguishing which of the three is
    /// actually responsible is the entire point of this diagnostic.
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_has_urls: std::cell::Cell<u64>,
    /// `scroll_offset_nonzero` fired count (pane-resolved calls only).
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_scroll_offset_nonzero: std::cell::Cell<u64>,
    /// `gutter_active` fired count (pane-resolved calls only).
    #[cfg(feature = "frame-profiling")]
    pub(super) pointer_cond_gutter_active: std::cell::Cell<u64>,
}

/// Task 121 pointer-motion repaint-gate diagnostic (feature-gated): which of
/// the eight conditions considered by `App::pointer_motion_needs_repaint`
/// were true for one call.
///
/// Distinct from `app_impl.rs`'s `PointerMotionPaneSignals`: that struct
/// feeds the actual repaint DECISION for a resolved pane (two aggregated
/// bools — `mouse_tracking_active` and a single `hover_region_risk`); this
/// struct is diagnostic-only, exhaustive over every named condition in the
/// predicate (including the three individual sub-terms
/// `pane_hover_region_risk` ORs together), and never affects behavior — it
/// is only ever consumed by `FrameStats::record_pointer_motion_check`.
///
/// The last four fields (`mouse_tracking_active` through `gutter_active`)
/// are meaningful only when a pane resolved under the pointer; when no pane
/// resolves, all four are simply left `false` (mirrors
/// `pointer_motion_needs_repaint_decision`'s own "no pane, so no
/// pane-specific signal applies" semantics for its `pane_signals: None`
/// case — NOT the same as `pointer_pane_unresolved`, which covers "the pane
/// could not be determined AT ALL").
// struct_excessive_bools: each field is an independent yes/no observation
// of one named condition in `pointer_motion_needs_repaint_decision`'s
// disjunction (or one of `pane_hover_region_risk`'s three sub-terms) — not
// a state machine. Combining them would not express any real combined
// state and would only obscure the one-flag-per-condition mapping this
// diagnostic exists to preserve.
#[cfg(feature = "frame-profiling")]
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct PointerMotionConditionFlags {
    pub(super) chrome_interactive: bool,
    pub(super) any_pane_selecting: bool,
    pub(super) overlay_open: bool,
    pub(super) pointer_pane_unresolved: bool,
    pub(super) mouse_tracking_active: bool,
    pub(super) has_urls: bool,
    pub(super) scroll_offset_nonzero: bool,
    pub(super) gutter_active: bool,
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

    /// Format the non-zero `(name, count)` entries as a single `name=count`
    /// comma-joined string for one `tracing` field, rather than one structured
    /// field per counter (which would make the already-busy frame-profiling
    /// line unreadable on the vast majority of frames where only one or two
    /// counters are ever non-zero).
    ///
    /// Takes pre-paired entries rather than parallel `names`/`counts` slices:
    /// with parallel arrays, reordering one without the other silently
    /// mislabels every value, and nothing in the type system catches it.
    /// Callers build the pairs from a single source of truth
    /// (`ChromeSignals::named_fields`, `Self::pointer_condition_counts`).
    ///
    /// Returns the literal string `"none"` when every count is zero. Pure, so
    /// directly unit-testable.
    pub(super) fn format_nonzero_counts(entries: &[(&str, u64)]) -> String {
        let parts: Vec<String> = entries
            .iter()
            .filter(|(_, count)| *count > 0)
            .map(|(name, count)| format!("{name}={count}"))
            .collect();
        if parts.is_empty() {
            "none".to_string()
        } else {
            parts.join(",")
        }
    }

    /// Task 121 pointer-motion repaint-gate diagnostic: record one
    /// `App::pointer_motion_needs_repaint` call's condition flags into the
    /// per-condition counters plus the total, `saturating_add`. Takes
    /// `&self` (not `&mut self`): every counter it touches is a
    /// `Cell<u64>` for exactly this reason — see
    /// `pointer_repaint_check_total`'s field doc.
    pub(super) fn record_pointer_motion_check(&self, flags: PointerMotionConditionFlags) {
        self.pointer_repaint_check_total
            .set(self.pointer_repaint_check_total.get().saturating_add(1));
        Self::bump_if(
            &self.pointer_cond_chrome_interactive,
            flags.chrome_interactive,
        );
        Self::bump_if(
            &self.pointer_cond_any_pane_selecting,
            flags.any_pane_selecting,
        );
        Self::bump_if(&self.pointer_cond_overlay_open, flags.overlay_open);
        Self::bump_if(
            &self.pointer_cond_pointer_pane_unresolved,
            flags.pointer_pane_unresolved,
        );
        Self::bump_if(
            &self.pointer_cond_mouse_tracking_active,
            flags.mouse_tracking_active,
        );
        Self::bump_if(&self.pointer_cond_has_urls, flags.has_urls);
        Self::bump_if(
            &self.pointer_cond_scroll_offset_nonzero,
            flags.scroll_offset_nonzero,
        );
        Self::bump_if(&self.pointer_cond_gutter_active, flags.gutter_active);
    }

    /// `counter.set(counter.get().saturating_add(1))` iff `condition` —
    /// the shared increment-a-`Cell`-iff-true step every branch of
    /// `record_pointer_motion_check` needs.
    fn bump_if(counter: &std::cell::Cell<u64>, condition: bool) {
        if condition {
            counter.set(counter.get().saturating_add(1));
        }
    }

    /// Format eight named pointer-motion condition counts the same way
    /// `format_nonzero_signal_counts` formats `chrome_signal_fired_counts`
    /// (see that method's doc for the full rationale) — `name=count`
    /// comma-joined, non-zero entries only, `"none"` when all eight are
    /// zero. A free-standing pure function over parallel `names`/`counts`
    /// arrays (not `&self`), so it is directly unit-testable without
    /// constructing a `Cell`-bearing `FrameStats`.
    /// Reset the pointer-motion condition counters and the total call
    /// counter (see `record_pointer_motion_check`) back to zero at the end
    /// of a flush window — see `pointer_repaint_check_total`'s field doc
    /// for why these are windowed rather than cumulative-since-creation.
    /// The eight pointer-motion condition counters paired with their names, in
    /// declaration order.
    ///
    /// Exists so the names and the counter reads cannot drift apart: keeping
    /// them as two hand-maintained parallel lists at the `tracing` call site
    /// means reordering one silently mislabels every value, with nothing in the
    /// type system to catch it. Mirrors
    /// `chrome_damage::ChromeSignals::named_fields()`, which solves the same
    /// problem for the chrome signals.
    pub(super) const fn pointer_condition_counts(&self) -> [(&'static str, u64); 8] {
        [
            (
                "chrome_interactive",
                self.pointer_cond_chrome_interactive.get(),
            ),
            (
                "any_pane_selecting",
                self.pointer_cond_any_pane_selecting.get(),
            ),
            ("overlay_open", self.pointer_cond_overlay_open.get()),
            (
                "pointer_pane_unresolved",
                self.pointer_cond_pointer_pane_unresolved.get(),
            ),
            (
                "mouse_tracking_active",
                self.pointer_cond_mouse_tracking_active.get(),
            ),
            ("has_urls", self.pointer_cond_has_urls.get()),
            (
                "scroll_offset_nonzero",
                self.pointer_cond_scroll_offset_nonzero.get(),
            ),
            ("gutter_active", self.pointer_cond_gutter_active.get()),
        ]
    }

    pub(super) fn reset_pointer_condition_window(&self) {
        self.pointer_repaint_check_total.set(0);
        self.pointer_cond_chrome_interactive.set(0);
        self.pointer_cond_any_pane_selecting.set(0);
        self.pointer_cond_overlay_open.set(0);
        self.pointer_cond_pointer_pane_unresolved.set(0);
        self.pointer_cond_mouse_tracking_active.set(0);
        self.pointer_cond_has_urls.set(0);
        self.pointer_cond_scroll_offset_nonzero.set(0);
        self.pointer_cond_gutter_active.set(0);
    }
}

#[cfg(all(test, feature = "frame-profiling"))]
mod frame_profiling_tests {
    use super::FrameStats;
    use std::time::Duration;

    /// Zip parallel name/count fixtures into the `(name, count)` pairs
    /// `format_nonzero_counts` now takes. Test-only: production callers build
    /// their pairs from a single source of truth (`named_fields`,
    /// `pointer_condition_counts`) precisely so no parallel arrays exist there.
    fn pairs<'a, const N: usize>(names: &[&'a str; N], counts: &[u64; N]) -> [(&'a str, u64); N] {
        std::array::from_fn(|i| (names[i], counts[i]))
    }

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

    // ── `FrameStats::format_nonzero_signal_counts` ───────────────────────

    /// A representative 15-name array; the actual names don't matter to
    /// this pure formatting helper, only that names/counts are parallel.
    const NAMES: [&str; 15] = [
        "any_overlay_open",
        "style_changed",
        "active_pane_changed",
        "tab_set_changed",
        "tab_title_changed",
        "pane_layout_changed",
        "broadcast_state_changed",
        "shader_active",
        "bell_active",
        "toast_active",
        "size_changed",
        "ppp_changed",
        "focus_changed",
        "warming_up",
        "foreground_overlay_open",
    ];

    #[test]
    fn format_nonzero_counts_all_zero_is_none() {
        let counts = [0u64; 15];
        assert_eq!(
            FrameStats::format_nonzero_counts(&pairs(&NAMES, &counts)),
            "none"
        );
    }

    #[test]
    fn format_nonzero_counts_shows_only_the_nonzero_entries() {
        let mut counts = [0u64; 15];
        counts[1] = 3; // style_changed
        counts[13] = 120; // warming_up
        assert_eq!(
            FrameStats::format_nonzero_counts(&pairs(&NAMES, &counts)),
            "style_changed=3,warming_up=120"
        );
    }

    #[test]
    fn format_nonzero_counts_preserves_declaration_order() {
        let mut counts = [0u64; 15];
        counts[14] = 1; // foreground_overlay_open
        counts[0] = 1; // any_overlay_open
        assert_eq!(
            FrameStats::format_nonzero_counts(&pairs(&NAMES, &counts)),
            "any_overlay_open=1,foreground_overlay_open=1",
            "order must follow the array's index order, not insertion order"
        );
    }

    // ── Task 121 pointer-motion repaint-gate condition counters ──────────

    /// The eight Task 121 pointer-motion condition names, in the same
    /// order `record_pointer_motion_check`/the app-side flush build their
    /// parallel `counts` array.
    const POINTER_CONDITION_NAMES: [&str; 8] = [
        "chrome_interactive",
        "any_pane_selecting",
        "overlay_open",
        "pointer_pane_unresolved",
        "mouse_tracking_active",
        "has_urls",
        "scroll_offset_nonzero",
        "gutter_active",
    ];

    #[test]
    fn format_nonzero_pointer_condition_counts_all_zero_is_none() {
        let counts = [0u64; 8];
        assert_eq!(
            FrameStats::format_nonzero_counts(&pairs(&POINTER_CONDITION_NAMES, &counts)),
            "none"
        );
    }

    #[test]
    fn format_nonzero_pointer_condition_counts_shows_only_the_nonzero_entries() {
        let mut counts = [0u64; 8];
        counts[4] = 12; // mouse_tracking_active
        counts[5] = 3; // has_urls
        assert_eq!(
            FrameStats::format_nonzero_counts(&pairs(&POINTER_CONDITION_NAMES, &counts)),
            "mouse_tracking_active=12,has_urls=3"
        );
    }

    #[test]
    fn format_nonzero_pointer_condition_counts_preserves_declaration_order() {
        let mut counts = [0u64; 8];
        counts[7] = 1; // gutter_active
        counts[0] = 1; // chrome_interactive
        assert_eq!(
            FrameStats::format_nonzero_counts(&pairs(&POINTER_CONDITION_NAMES, &counts)),
            "chrome_interactive=1,gutter_active=1",
            "order must follow the array's index order, not insertion order"
        );
    }

    #[test]
    fn record_pointer_motion_check_increments_total_and_only_true_conditions() {
        use super::PointerMotionConditionFlags;

        let stats = FrameStats::default();
        stats.record_pointer_motion_check(PointerMotionConditionFlags {
            chrome_interactive: true,
            any_pane_selecting: false,
            overlay_open: false,
            pointer_pane_unresolved: false,
            mouse_tracking_active: false,
            has_urls: true,
            scroll_offset_nonzero: false,
            gutter_active: false,
        });

        assert_eq!(stats.pointer_repaint_check_total.get(), 1);
        assert_eq!(stats.pointer_cond_chrome_interactive.get(), 1);
        assert_eq!(stats.pointer_cond_any_pane_selecting.get(), 0);
        assert_eq!(stats.pointer_cond_overlay_open.get(), 0);
        assert_eq!(stats.pointer_cond_pointer_pane_unresolved.get(), 0);
        assert_eq!(stats.pointer_cond_mouse_tracking_active.get(), 0);
        assert_eq!(stats.pointer_cond_has_urls.get(), 1);
        assert_eq!(stats.pointer_cond_scroll_offset_nonzero.get(), 0);
        assert_eq!(stats.pointer_cond_gutter_active.get(), 0);
    }

    #[test]
    fn record_pointer_motion_check_counts_each_condition_independently() {
        use super::PointerMotionConditionFlags;

        // All eight conditions true at once must all be counted -- the
        // task's explicit requirement ("several can be true at once --
        // count each independently, do not stop at the first").
        let stats = FrameStats::default();
        stats.record_pointer_motion_check(PointerMotionConditionFlags {
            chrome_interactive: true,
            any_pane_selecting: true,
            overlay_open: true,
            pointer_pane_unresolved: true,
            mouse_tracking_active: true,
            has_urls: true,
            scroll_offset_nonzero: true,
            gutter_active: true,
        });

        assert_eq!(stats.pointer_repaint_check_total.get(), 1);
        assert_eq!(stats.pointer_cond_chrome_interactive.get(), 1);
        assert_eq!(stats.pointer_cond_any_pane_selecting.get(), 1);
        assert_eq!(stats.pointer_cond_overlay_open.get(), 1);
        assert_eq!(stats.pointer_cond_pointer_pane_unresolved.get(), 1);
        assert_eq!(stats.pointer_cond_mouse_tracking_active.get(), 1);
        assert_eq!(stats.pointer_cond_has_urls.get(), 1);
        assert_eq!(stats.pointer_cond_scroll_offset_nonzero.get(), 1);
        assert_eq!(stats.pointer_cond_gutter_active.get(), 1);
    }

    #[test]
    fn reset_pointer_condition_window_clears_every_counter() {
        use super::PointerMotionConditionFlags;

        let stats = FrameStats::default();
        stats.record_pointer_motion_check(PointerMotionConditionFlags {
            chrome_interactive: true,
            any_pane_selecting: true,
            overlay_open: true,
            pointer_pane_unresolved: true,
            mouse_tracking_active: true,
            has_urls: true,
            scroll_offset_nonzero: true,
            gutter_active: true,
        });

        stats.reset_pointer_condition_window();

        assert_eq!(stats.pointer_repaint_check_total.get(), 0);
        assert_eq!(stats.pointer_cond_chrome_interactive.get(), 0);
        assert_eq!(stats.pointer_cond_any_pane_selecting.get(), 0);
        assert_eq!(stats.pointer_cond_overlay_open.get(), 0);
        assert_eq!(stats.pointer_cond_pointer_pane_unresolved.get(), 0);
        assert_eq!(stats.pointer_cond_mouse_tracking_active.get(), 0);
        assert_eq!(stats.pointer_cond_has_urls.get(), 0);
        assert_eq!(stats.pointer_cond_scroll_offset_nonzero.get(), 0);
        assert_eq!(stats.pointer_cond_gutter_active.get(), 0);
    }
}

#[cfg(test)]
mod resize_overlay_tests {
    use super::{
        RESIZE_OVERLAY_FADE, RESIZE_OVERLAY_LINGER, resize_is_genuine, resize_overlay_alpha,
        resize_overlay_is_animating, resize_overlay_repaint_delay,
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

    // ── Subtask 121.14: `resize_overlay_is_animating` ────────────────────

    #[test]
    fn is_animating_false_in_the_opaque_phase() {
        // linger=900ms, fade=250ms -> fade starts at elapsed=650ms.
        let elapsed = RESIZE_OVERLAY_LINGER
            .saturating_sub(RESIZE_OVERLAY_FADE)
            .checked_sub(Duration::from_millis(1))
            .unwrap_or(Duration::ZERO);
        assert!(!resize_overlay_is_animating(
            elapsed,
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE
        ));
    }

    #[test]
    fn is_animating_true_at_the_exact_fade_start_boundary() {
        let fade_start = RESIZE_OVERLAY_LINGER.saturating_sub(RESIZE_OVERLAY_FADE);
        assert!(resize_overlay_is_animating(
            fade_start,
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE
        ));
    }

    #[test]
    fn is_animating_true_through_the_fade() {
        let elapsed = RESIZE_OVERLAY_LINGER.saturating_sub(RESIZE_OVERLAY_FADE / 2);
        assert!(resize_overlay_is_animating(
            elapsed,
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE
        ));
    }

    #[test]
    fn is_animating_true_past_linger() {
        // The load-bearing case: the overlay is only cleared by a rendered
        // frame, so this must stay `true` past `linger`, not fall back to
        // `false` — see this function's doc for what would be stranded if
        // it did not.
        assert!(resize_overlay_is_animating(
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE
        ));
        assert!(resize_overlay_is_animating(
            RESIZE_OVERLAY_LINGER + Duration::from_secs(5),
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_FADE
        ));
    }

    #[test]
    fn is_animating_with_zero_fade_flips_exactly_at_linger() {
        // fade=0 -> fade_start == linger: opaque for the entire linger
        // window, then immediately "animating" (the instantaneous snap to
        // invisible) at/after linger.
        assert!(!resize_overlay_is_animating(
            RESIZE_OVERLAY_LINGER
                .checked_sub(Duration::from_millis(1))
                .unwrap_or(Duration::ZERO),
            RESIZE_OVERLAY_LINGER,
            Duration::ZERO
        ));
        assert!(resize_overlay_is_animating(
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_LINGER,
            Duration::ZERO
        ));
    }

    #[test]
    fn is_animating_with_fade_longer_than_linger_is_always_animating() {
        // fade > linger -> `linger.saturating_sub(fade)` saturates to ZERO,
        // so every `elapsed >= 0` (i.e. always) reads as animating.
        let fade = RESIZE_OVERLAY_LINGER + Duration::from_secs(1);
        assert!(resize_overlay_is_animating(
            Duration::ZERO,
            RESIZE_OVERLAY_LINGER,
            fade
        ));
        assert!(resize_overlay_is_animating(
            RESIZE_OVERLAY_LINGER,
            RESIZE_OVERLAY_LINGER,
            fade
        ));
    }

    // ── Subtask 121.14: `resize_overlay_repaint_delay` ───────────────────

    #[test]
    fn repaint_delay_in_the_opaque_phase_wakes_exactly_at_fade_start() {
        let elapsed = Duration::ZERO;
        let fade_start = RESIZE_OVERLAY_LINGER.saturating_sub(RESIZE_OVERLAY_FADE);
        assert_eq!(
            resize_overlay_repaint_delay(elapsed, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE),
            fade_start
        );

        // Partway through the opaque phase: delay is the remaining time to
        // fade-start, not the whole window.
        let elapsed = Duration::from_millis(100);
        assert_eq!(
            resize_overlay_repaint_delay(elapsed, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE),
            fade_start.saturating_sub(elapsed)
        );
    }

    #[test]
    fn repaint_delay_once_animating_is_16ms() {
        let fade_start = RESIZE_OVERLAY_LINGER.saturating_sub(RESIZE_OVERLAY_FADE);
        assert_eq!(
            resize_overlay_repaint_delay(fade_start, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE),
            Duration::from_millis(16)
        );
        assert_eq!(
            resize_overlay_repaint_delay(
                RESIZE_OVERLAY_LINGER,
                RESIZE_OVERLAY_LINGER,
                RESIZE_OVERLAY_FADE
            ),
            Duration::from_millis(16)
        );
        assert_eq!(
            resize_overlay_repaint_delay(
                RESIZE_OVERLAY_LINGER + Duration::from_secs(5),
                RESIZE_OVERLAY_LINGER,
                RESIZE_OVERLAY_FADE
            ),
            Duration::from_millis(16)
        );
    }

    #[test]
    fn repaint_delay_boundary_matches_is_animating_boundary() {
        // The two functions must agree at every boundary they share -- this
        // is the "MUST stay consistent" invariant from both docs, pinned
        // directly: wherever `resize_overlay_is_animating` is `true`, the
        // delay must be exactly 16ms; wherever it is `false` (the opaque
        // phase), the delay must be exactly the time remaining until
        // fade-start -- which can itself be arbitrarily small (e.g. 1ms
        // just before the boundary), so the two functions are compared
        // against the same formula rather than an inequality against 16ms.
        let fade_start = RESIZE_OVERLAY_LINGER.saturating_sub(RESIZE_OVERLAY_FADE);
        for millis in [0, 1, 100, 649, 650, 651, 899, 900, 901, 5000] {
            let elapsed = Duration::from_millis(millis);
            let animating =
                resize_overlay_is_animating(elapsed, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE);
            let delay =
                resize_overlay_repaint_delay(elapsed, RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE);
            if animating {
                assert_eq!(delay, Duration::from_millis(16), "elapsed={elapsed:?}");
            } else {
                assert_eq!(
                    delay,
                    fade_start.saturating_sub(elapsed),
                    "elapsed={elapsed:?} delay={delay:?}"
                );
            }
        }
    }

    #[test]
    fn repaint_delay_with_fade_longer_than_linger_is_always_16ms() {
        let fade = RESIZE_OVERLAY_LINGER + Duration::from_secs(1);
        assert_eq!(
            resize_overlay_repaint_delay(Duration::ZERO, RESIZE_OVERLAY_LINGER, fade),
            Duration::from_millis(16)
        );
    }

    // ── Item 4 (review CONSIDER #4): pin the HUD consistency invariant ────

    #[test]
    fn resize_overlay_is_animating_and_repaint_delay_stay_consistent_across_ranges() {
        // The two functions' docs call their mutual consistency
        // "load-bearing" (disagreement either janks the HUD or never lets it
        // sleep) but share the `fade_start = linger.saturating_sub(fade)`
        // formula only by duplication -- nothing previously caught a future
        // edit to one without the other across anything but the production
        // LINGER/FADE constants (`repaint_delay_boundary_matches_is_animating_boundary`
        // above). This generalizes that check across several linger/fade
        // combinations, including the edge cases the review explicitly
        // called out: `fade == 0` and `fade > linger`.
        //
        // CORRECTNESS NOTE on the invariant actually pinned here: the task
        // that produced this test described the invariant as
        // `is_animating(e, l, f) == (repaint_delay(e, l, f) <= 16ms)`. That
        // formulation is subtly wrong and is deliberately NOT what is
        // asserted below. Counterexample: with `linger = 900ms`,
        // `fade = 250ms` (`fade_start = 650ms`), at `elapsed = 640ms`,
        // `resize_overlay_is_animating` is `false` (still opaque, `640 <
        // 650`) but `resize_overlay_repaint_delay` returns `10ms`, which
        // is `<= 16ms` -- the opaque-phase delay counts down toward
        // `fade_start` and is transiently small in the last <=16ms before
        // that boundary, with `is_animating` still `false` the whole time.
        // A literal `<= 16ms` comparison would therefore fail on that
        // legitimate, correctly-behaving case. The REAL, load-bearing
        // coupling -- confirmed against both functions' bodies -- is the
        // exact formula each doc already claims:
        //   - `animating`  => `repaint_delay == 16ms` (exactly, the fast
        //     cadence)
        //   - `!animating` => `repaint_delay == fade_start.saturating_sub(elapsed)`
        //     (the countdown to fade-start, which may itself be smaller
        //     than 16ms near the boundary)
        // That is what is pinned below, per-combination and per-sample.
        let combos = [
            (RESIZE_OVERLAY_LINGER, RESIZE_OVERLAY_FADE), // production defaults
            (Duration::from_millis(900), Duration::ZERO), // fade == 0
            (Duration::from_millis(900), Duration::from_millis(900)), // fade == linger
            (Duration::from_millis(500), Duration::from_millis(900)), // fade > linger
            (Duration::ZERO, Duration::ZERO),             // degenerate: both zero
        ];

        for (linger, fade) in combos {
            let fade_start = linger.saturating_sub(fade);
            let mut samples = vec![
                Duration::ZERO,
                fade_start.saturating_sub(Duration::from_millis(1)),
                fade_start,
                fade_start + Duration::from_millis(1),
                linger.saturating_sub(Duration::from_millis(1)),
                linger,
                linger + Duration::from_millis(1),
                linger + Duration::from_secs(5),
            ];
            samples.sort();
            samples.dedup();

            for elapsed in samples {
                let animating = resize_overlay_is_animating(elapsed, linger, fade);
                let delay = resize_overlay_repaint_delay(elapsed, linger, fade);
                if animating {
                    assert_eq!(
                        delay,
                        Duration::from_millis(16),
                        "linger={linger:?} fade={fade:?} elapsed={elapsed:?}: \
                         animating must always request exactly 16ms"
                    );
                } else {
                    assert_eq!(
                        delay,
                        fade_start.saturating_sub(elapsed),
                        "linger={linger:?} fade={fade:?} elapsed={elapsed:?}: \
                         opaque phase must request exactly the countdown to fade-start"
                    );
                }
            }
        }
    }
}
