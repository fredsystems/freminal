// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

use std::sync::Arc;
use std::time::Instant;

use arboard::Clipboard;
use conv2::ConvUtil;
use crossbeam_channel::{Receiver, Sender};
use egui::{self, Pos2, Vec2, ViewportCommand};
use freminal_common::base64::encode;
use freminal_common::buffer_states::window_manipulation::WindowManipulation;
use freminal_common::config::BellMode;
use freminal_common::gui_theme::GuiTheme;
use freminal_common::send_or_log;
use freminal_common::themes::ThemePalette;
use freminal_terminal_emulator::ansi_components::tracer::escape_sequence_for_log_bounded;
use freminal_terminal_emulator::io::{GuiReply, InputEvent, WindowCommand, WindowStateReport};

use crate::gui::chrome_style;
use crate::gui::notifications::{NotificationRequest, Osc99Control};
use crate::gui::osc99::{
    Osc99ControlRequest, Osc99DisplayRequest, Osc99IconPrime, Osc99PaneContext, Osc99Request,
};
use crate::gui::title_stack::TitleStack;

/// Apply the full palette-derived chrome [`Visuals`](egui::Visuals) and
/// spacing for the initial / window-creation style setup.
///
/// Routes through [`chrome_style::build_visuals`](crate::gui::chrome_style::build_visuals)
/// so the entire chrome (widgets, borders, selection, menus) is themed, not
/// just `window_fill`/`panel_fill`.  Chrome styling never depends on any
/// pane's display mode (DECSCNM reverse video is a per-cell terminal-content
/// concern); the per-frame style hook in `app_impl::update` keeps the
/// visuals in sync with the active `GuiTheme` profile thereafter.
pub(super) fn set_egui_options(
    ctx: &egui::Context,
    theme: &ThemePalette,
    bg_opacity: f32,
    gui_theme: &GuiTheme,
) {
    apply_chrome_visuals(ctx, theme, bg_opacity, gui_theme);
    ctx.options_mut(|options| {
        options.zoom_with_keyboard = false;
    });
}

/// Re-apply the full palette-derived chrome [`Visuals`](egui::Visuals) to match
/// a new theme (e.g. an OS dark/light change in `Auto` theme mode).
pub(super) fn update_egui_theme(
    ctx: &egui::Context,
    theme: &ThemePalette,
    bg_opacity: f32,
    gui_theme: &GuiTheme,
) {
    apply_chrome_visuals(ctx, theme, bg_opacity, gui_theme);
}

/// Shared body for [`set_egui_options`] / [`update_egui_theme`]: derive the
/// chrome `Visuals` + spacing from the active palette and the active
/// [`GuiTheme`] profile (the user's persisted `[chrome] profile`, Task 112.13),
/// then apply them globally.
///
/// Chrome styling is independent of any pane's display mode; see
/// [`set_egui_options`] doc comment.
fn apply_chrome_visuals(
    ctx: &egui::Context,
    theme: &ThemePalette,
    bg_opacity: f32,
    gui_theme: &GuiTheme,
) {
    let visuals = chrome_style::build_visuals(gui_theme, theme, bg_opacity);
    ctx.global_style_mut(|style| {
        style.visuals = visuals;
        chrome_style::apply_chrome_spacing(style, gui_theme);
    });
}

/// Send a structured GUI reply to the pane's PTY thread.
///
/// Used by `handle_window_manipulation` to answer Report* and OSC 52 queries.
/// The PTY thread serialises the reply and frames it according to the
/// application's S8C1T mode, so the GUI never formats escape-sequence bytes.
fn send_gui_reply(reply_tx: &Sender<InputEvent>, reply: GuiReply) {
    send_or_log!(
        reply_tx,
        InputEvent::Reply(reply),
        "Failed to send GUI reply to the PTY thread"
    );
}

/// Read the system clipboard and return its contents as a base64-encoded string.
///
/// Returns an empty string on any error (clipboard unavailable, empty, etc.).
/// This is intentionally infallible — clipboard access is best-effort.
///
/// Clipboard contents beyond [`MAX_CLIPBOARD_BYTES`] are truncated to avoid
/// excessive memory allocation and PTY traffic from a large clipboard.
pub(super) fn read_clipboard_base64() -> String {
    /// Maximum clipboard payload size (bytes) returned for OSC 52 queries.
    /// 100 KiB matches limits used by other terminal emulators (e.g. xterm).
    const MAX_CLIPBOARD_BYTES: usize = 100 * 1024;

    let Ok(mut clipboard) = Clipboard::new() else {
        debug!("OSC 52 query: failed to open clipboard");
        return String::new();
    };

    match clipboard.get_text() {
        Ok(text) if !text.is_empty() => {
            let bytes = text.as_bytes();
            if bytes.len() > MAX_CLIPBOARD_BYTES {
                debug!(
                    "OSC 52 query: clipboard truncated from {} to {MAX_CLIPBOARD_BYTES} bytes",
                    bytes.len()
                );
                encode(&bytes[..MAX_CLIPBOARD_BYTES])
            } else {
                encode(bytes)
            }
        }
        Ok(_) => String::new(),
        Err(e) => {
            debug!("OSC 52 query: clipboard read error: {e}");
            String::new()
        }
    }
}

/// Flags for [`handle_window_manipulation`] that control which commands are
/// honoured for a given pane.
#[allow(clippy::struct_excessive_bools)]
pub(super) struct WindowManipFlags {
    /// Whether clipboard read requests are allowed by policy.
    pub allow_clipboard_read: bool,
    /// Whether this pane is the fully-active pane (active tab + active pane).
    pub is_active: bool,
    /// Whether the window currently has OS focus.
    pub window_focused: bool,
    /// Whether this pane is the only pane in its tab (no splits).
    pub is_only_pane: bool,
    /// Where this pane's OSC 99 requests come from and the pane's state this
    /// frame, stamped onto every collected OSC 99 request.
    pub osc99: Osc99PaneContext,
}

/// An OSC 52 clipboard event observed during a frame's window-manipulation
/// drain, collected so the caller can surface a toast after the per-pane
/// loop releases its `win.tabs` borrow (the handler itself has no access to
/// the app-level config/toast stack). See
/// [`FreminalToastCategory::ClipboardRemote`].
///
/// [`FreminalToastCategory::ClipboardRemote`]: freminal_common::config::FreminalToastCategory::ClipboardRemote
#[derive(Debug, Clone)]
pub(super) enum Osc52ToastEvent {
    /// A terminal application wrote text to the system clipboard (OSC 52
    /// set). Carries the byte length of the written payload for the toast
    /// detail (never the content itself — do not leak clipboard data into a
    /// toast).
    Wrote { bytes: usize },
    /// A terminal application attempted to READ the system clipboard (OSC 52
    /// query) but was blocked by `[security] allow_clipboard_read = false`.
    ReadBlocked,
}

/// Maps an [`Osc52ToastEvent`] to the `(title, detail)` pair used for the
/// resulting toast. Pure and free of `egui`/config access so it can be
/// unit-tested directly; the caller (`app_impl::update()`) supplies the
/// category gating and the toast stack via `FreminalGui::route_freminal_toast`.
pub(super) fn osc52_toast_text(event: &Osc52ToastEvent) -> (&'static str, Option<String>) {
    match event {
        Osc52ToastEvent::Wrote { bytes } => (
            "Clipboard updated",
            Some(format!(
                "An application copied {bytes} bytes to the clipboard."
            )),
        ),
        Osc52ToastEvent::ReadBlocked => (
            "Clipboard read blocked",
            Some(
                "An application tried to read the clipboard; blocked by your security settings."
                    .to_owned(),
            ),
        ),
    }
}

/// Drain and dispatch all pending [`WindowCommand`]s for this frame.
///
/// ## Flow
///
/// 1. **Non-blocking drain** — `window_cmd_rx.try_recv()` is called in a
///    loop until the channel is empty.  All commands queued by the PTY
///    consumer thread since the last frame are processed before rendering.
///
/// 2. **Variant routing** — both `Viewport` and `Report` commands carry
///    the same inner `WindowManipulation` value; the outer tag is not used
///    for routing here (the dispatch is done entirely on the inner value).
///
/// 3. **Viewport operations** — forwarded to egui via
///    `ui.ctx().send_viewport_cmd(ViewportCommand::…)`.  Covers move,
///    resize, minimize/restore, maximize/restore, fullscreen, raise/lower,
///    de-iconify, and resize-to-lines-and-columns.
///
/// 4. **Report queries** — the function measures the current viewport
///    geometry from `ui.ctx()` (pixel positions, sizes) and the font metrics
///    (`font_width`, `font_height`), then sends the answer to the pane's PTY
///    thread as a structured [`GuiReply`] on `reply_tx` (the pane's shared
///    handle to its input channel, wrapped in [`InputEvent::Reply`]).  The
///    PTY thread serialises the reply and frames it per the application's
///    S8C1T mode; the GUI never formats escape-sequence bytes.  Covered
///    variants:
///    - `ReportWindowState` → `GuiReply::WindowState` (`CSI 1 t` / `CSI 2 t`)
///    - `ReportWindowPosition*` → `GuiReply::WindowPosition` (`CSI 3 ; x ; y t`)
///    - `ReportWindowSize*` → `GuiReply::WindowSizePixels` (`CSI 4 ; h ; w t`)
///    - `ReportRootWindowSizeInPixels` → `GuiReply::ScreenSizePixels`
///      (`CSI 5 ; h ; w t`)
///    - `ReportIconLabel` → `GuiReply::IconLabel` (`OSC L <label> ST`)
///    - `ReportTitle` → `GuiReply::WindowTitle` (`OSC l <title> ST`)
///
///    **Not handled here** (no-ops in this function):
///    - `ReportCharacterSizeInPixels`, `ReportTerminalSizeInCharacters`,
///      `ReportRootWindowSizeInCharacters` — these are handled synchronously
///      on the PTY thread by `TerminalHandler::handle_window_manipulation` so
///      that responses arrive in the same batch as DA1.  They never reach the
///      GUI's `window_cmd_rx` stream.
///
/// 5. **Title stack** — `SaveWindowTitleToStack` and
///    `RestoreWindowTitleFromStack` push/pop from `title_stack`; `SetTitleBarText`
///    calls `ViewportCommand::Title`.
///
/// 6. **OSC 52 clipboard** — `SetClipboard` copies decoded text to the system
///    clipboard via `ui.ctx().copy_text()`.  `QueryClipboard` reads the system
///    clipboard via `arboard` when `allow_clipboard_read` is `true`; otherwise
///    it responds with an empty payload (the safe/secure default).
// Inherently large: handles all `WindowCommand` variants — viewport commands, Report* PTY
// responses, title stack, clipboard. Each variant requires distinct context (ui, reply_tx,
// title_stack). Splitting further would scatter a cohesive protocol handler.
// All arguments are required context that cannot be easily grouped without obscuring intent.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(super) fn handle_window_manipulation(
    ui: &egui::Ui,
    window_cmd_rx: &Receiver<WindowCommand>,
    reply_tx: &Arc<Sender<InputEvent>>,
    font_width: usize,
    font_height: usize,
    window_width: egui::Rect,
    title_stack: &mut TitleStack,
    tab_title: &mut String,
    bell_active: &mut bool,
    bell_since: &mut Option<Instant>,
    bell_mode: BellMode,
    flags: &WindowManipFlags,
    notifications: &mut Vec<NotificationRequest>,
    osc99_requests: &mut Vec<Osc99Request>,
    osc52_events: &mut Vec<Osc52ToastEvent>,
) -> bool {
    // Whether the shell set (or restored) a title during this frame.  Used
    // by the caller to clear any user-assigned custom tab name, so
    // shell-driven titles take precedence once the user stops pinning a
    // rename.  Returned at end of function.
    let mut shell_set_title = false;
    // Drain all pending WindowCommands for this frame.
    while let Ok(wc) = window_cmd_rx.try_recv() {
        let window_event = match wc {
            WindowCommand::Viewport(cmd) | WindowCommand::Report(cmd) => cmd,
        };

        match window_event {
            // ── Viewport-mutating commands: skip for inactive tabs ───
            // An inactive tab must not resize, move, minimize, or fullscreen
            // the shared window.
            WindowManipulation::DeIconifyWindow
            | WindowManipulation::MinimizeWindow
            | WindowManipulation::MoveWindow(_, _)
            | WindowManipulation::ResizeWindow(_, _)
            | WindowManipulation::MaximizeWindow
            | WindowManipulation::RestoreNonMaximizedWindow
            | WindowManipulation::ResizeWindowToLinesAndColumns(_, _)
            | WindowManipulation::NotFullScreen
            | WindowManipulation::FullScreen
            | WindowManipulation::ToggleFullScreen
                if !flags.is_active => {}

            // ── Resize commands from split panes: suppress ───────────
            // A pane sharing space in a split must not resize the OS
            // window to its own dimensions — that would shrink the
            // window to a fraction of its intended size.
            WindowManipulation::ResizeWindow(_, _)
            | WindowManipulation::ResizeWindowToLinesAndColumns(_, _)
                if !flags.is_only_pane => {}

            // ── Title: inactive tabs update their own title only ─────
            WindowManipulation::SetTitleBarText(title) if !flags.is_active => {
                tab_title.clone_from(&title);
                shell_set_title = true;
            }

            // ── Title stack: inactive tabs save their own tab title ──
            WindowManipulation::SaveWindowTitleToStack if !flags.is_active => {
                title_stack.push(tab_title.clone());
            }
            WindowManipulation::RestoreWindowTitleFromStack if !flags.is_active => {
                if let Some(title) = title_stack.pop() {
                    tab_title.clone_from(&title);
                } else {
                    tab_title.clear();
                }
                shell_set_title = true;
            }
            WindowManipulation::DeIconifyWindow => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Minimized(false));
            }
            WindowManipulation::MinimizeWindow => {
                ui.ctx().send_viewport_cmd(ViewportCommand::Minimized(true));
            }
            WindowManipulation::MoveWindow(x, y) => {
                let x = x.approx_as::<f32>().unwrap_or_default();
                let y = y.approx_as::<f32>().unwrap_or_default();

                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(x, y)));
            }
            WindowManipulation::ResizeWindow(width, height) => {
                let width = width.approx_as::<f32>().unwrap_or_default();
                let height = height.approx_as::<f32>().unwrap_or_default();

                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(width, height)));
            }
            WindowManipulation::MaximizeWindow => {
                ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(true));
            }
            WindowManipulation::RestoreNonMaximizedWindow => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Maximized(false));
            }
            WindowManipulation::ResizeWindowToLinesAndColumns(input_height, input_width) => {
                let available_height = ui.available_height();
                let available_width = ui.available_width();
                let width_difference = window_width.width() - available_width;
                let height_difference = window_width.height() - available_height;
                let width = input_width * font_width;
                let height = input_height * font_height;

                let width = width.approx_as::<f32>().unwrap_or_default() + width_difference;
                let height = height.approx_as::<f32>().unwrap_or_default() + height_difference;

                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::InnerSize(Vec2::new(width, height)));
            }
            WindowManipulation::NotFullScreen => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Fullscreen(false));
            }
            WindowManipulation::FullScreen => {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Fullscreen(true));
            }
            WindowManipulation::ToggleFullScreen => {
                let current_status = ui.ctx().input(|i| i.viewport().fullscreen.unwrap_or(false));
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::Fullscreen(!current_status));
            }
            WindowManipulation::ReportWindowState => {
                let minimized = ui.ctx().input(|i| i.viewport().minimized.unwrap_or(false));
                let state = if minimized {
                    WindowStateReport::Iconified
                } else {
                    WindowStateReport::Normal
                };
                send_gui_reply(reply_tx, GuiReply::WindowState(state));
            }
            WindowManipulation::ReportWindowPositionWholeWindow => {
                let position = ui
                    .ctx()
                    .input(|i| {
                        i.raw.viewport().outer_rect.unwrap_or_else(|| {
                            error!("Failed to get viewport position. Using 0 as default");
                            egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                        })
                    })
                    .min;

                let pos_x = position.x.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert position x to usize: {e}. Using 0 as default");
                    0
                });
                let pos_y = position.y.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert position y to usize: {e}. Using 0 as default");
                    0
                });

                send_gui_reply(reply_tx, GuiReply::WindowPosition { x: pos_x, y: pos_y });
            }
            WindowManipulation::ReportWindowPositionTextArea => {
                let position = ui
                    .ctx()
                    .input(|i| {
                        i.raw.viewport().outer_rect.unwrap_or_else(|| {
                            error!("Failed to get viewport position. Using 0 as default");
                            egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                        })
                    })
                    .min;

                let available_height = ui.available_height();
                let available_width = ui.available_width();
                let width_difference = window_width.width() - available_width;
                let height_difference = window_width.height() - available_height;
                let pos_x = (position.x + width_difference)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert position x to usize: {e}. Using 0 as default");
                        0
                    });
                let pos_y = (position.y + height_difference)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert position y to usize: {e}. Using 0 as default");
                        0
                    });

                send_gui_reply(reply_tx, GuiReply::WindowPosition { x: pos_x, y: pos_y });
            }
            WindowManipulation::ReportWindowSizeInPixels => {
                let rect = ui.ctx().input(|i| {
                    i.raw.viewport().outer_rect.unwrap_or_else(|| {
                        error!("Failed to get viewport position. Using 0 as default");
                        egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                    })
                });

                let width = (rect.max.x - rect.min.x)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert width to usize: {e}. Using 0 as default");
                        0
                    });
                let height = (rect.max.y - rect.min.y)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert height to usize: {e}. Using 0 as default");
                        0
                    });

                send_gui_reply(reply_tx, GuiReply::WindowSizePixels { height, width });
            }
            WindowManipulation::ReportWindowTextAreaSizeInPixels => {
                let size = ui.ctx().content_rect().max;
                let width = size.x.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert width to usize: {e}. Using 0 as default");
                    0
                });
                let height = size.y.approx_as::<usize>().unwrap_or_else(|e| {
                    error!("Failed to convert height to usize: {e}. Using 0 as default");
                    0
                });

                send_gui_reply(reply_tx, GuiReply::WindowSizePixels { height, width });
            }
            WindowManipulation::ReportRootWindowSizeInPixels => {
                let rect = ui.ctx().input(|i| {
                    i.raw.viewport().outer_rect.unwrap_or_else(|| {
                        error!("Failed to get viewport position. Using 0 as default");
                        egui::Rect::from_min_size(Pos2::new(0.0, 0.0), Vec2::new(0.0, 0.0))
                    })
                });

                let width = (rect.max.x - rect.min.x)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert width to usize: {e}. Using 0 as default");
                        0
                    });
                let height = (rect.max.y - rect.min.y)
                    .approx_as::<usize>()
                    .unwrap_or_else(|e| {
                        error!("Failed to convert height to usize: {e}. Using 0 as default");
                        0
                    });

                send_gui_reply(reply_tx, GuiReply::ScreenSizePixels { height, width });
            }
            // ReportCharacterSizeInPixels, ReportTerminalSizeInCharacters, and
            // ReportRootWindowSizeInCharacters are handled synchronously by the
            // PTY thread (TerminalHandler::handle_window_manipulation) so that
            // responses arrive in the same batch as DA1.  They never reach here.
            WindowManipulation::ReportCharacterSizeInPixels
            | WindowManipulation::ReportTerminalSizeInCharacters
            | WindowManipulation::ReportRootWindowSizeInCharacters => {}
            WindowManipulation::ReportIconLabel => {
                let title = ui.ctx().input(|r| r.raw.viewport().title.clone());
                let title = title.unwrap_or_else(|| {
                    error!("Failed to get viewport title. Using Freminal");
                    "Freminal".to_string()
                });
                send_gui_reply(reply_tx, GuiReply::IconLabel(title));
            }
            WindowManipulation::ReportTitle => {
                let title = ui.ctx().input(|r| r.raw.viewport().title.clone());
                let title = title.unwrap_or_else(|| {
                    error!("Failed to get viewport title. Using Freminal");
                    "Freminal".to_string()
                });
                send_gui_reply(reply_tx, GuiReply::WindowTitle(title));
            }
            WindowManipulation::SetTitleBarText(title) => {
                // Update the tab title for the tab bar display.
                tab_title.clone_from(&title);
                shell_set_title = true;
                // Set the window title bar to the active tab's title.
                ui.ctx()
                    .send_viewport_cmd(egui::ViewportCommand::Title(title));
            }
            WindowManipulation::SaveWindowTitleToStack => {
                let title = ui.ctx().input(|r| r.raw.viewport().title.clone());
                let title = title.unwrap_or_else(|| {
                    error!("Failed to get viewport title. Using Freminal");
                    "Freminal".to_string()
                });
                title_stack.push(title);
            }
            WindowManipulation::RestoreWindowTitleFromStack => {
                if let Some(title) = title_stack.pop() {
                    tab_title.clone_from(&title);
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title(title));
                } else {
                    tab_title.clear();
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Title("Freminal".to_string()));
                }
                shell_set_title = true;
            }
            // These are ignored. eGui doesn't give us a stacking order thing (that I can tell).
            // Refresh window is already happening because we ended up here.
            WindowManipulation::RefreshWindow
            | WindowManipulation::LowerWindowToBottomOfStackingOrder
            | WindowManipulation::RaiseWindowToTopOfStackingOrder => (),

            // OSC 52 clipboard set: copy decoded text to the system clipboard.
            WindowManipulation::SetClipboard(_sel, content) => {
                osc52_events.push(Osc52ToastEvent::Wrote {
                    bytes: content.len(),
                });
                ui.ctx().copy_text(content);
            }

            // OSC 52 clipboard query: read the system clipboard when the
            // user has opted in via [security] allow_clipboard_read = true.
            // Otherwise respond with an empty payload (safe default).
            WindowManipulation::QueryClipboard(sel) => {
                let payload = if flags.allow_clipboard_read {
                    read_clipboard_base64()
                } else {
                    // `sel` is application-supplied: escape and bound it.
                    tracing::debug!(
                        "OSC 52 query for selection '{}' — blocked by security config",
                        escape_sequence_for_log_bounded(sel.as_bytes())
                    );
                    osc52_events.push(Osc52ToastEvent::ReadBlocked);
                    String::new()
                };
                send_gui_reply(
                    reply_tx,
                    GuiReply::Clipboard {
                        selection: sel,
                        base64_payload: payload,
                    },
                );
            }

            // Terminal bell: dispatch to the visual and/or audio paths based
            // on the user's [bell] mode.  `None` is a silent drop.  Audio is
            // a best-effort system beep (see `gui::platform::system_beep`).
            // In every non-None case, request OS taskbar attention when the
            // window is unfocused so the user notices even off-screen.
            WindowManipulation::Bell => {
                let visual = matches!(bell_mode, BellMode::Visual | BellMode::Both);
                let audio = matches!(bell_mode, BellMode::Audio | BellMode::Both);

                if visual {
                    *bell_active = true;
                    *bell_since = Some(Instant::now());
                }

                if audio {
                    super::platform::system_beep();
                }

                if (visual || audio) && !flags.window_focused {
                    ui.ctx()
                        .send_viewport_cmd(ViewportCommand::RequestUserAttention(
                            egui::UserAttentionType::Informational,
                        ));
                }
            }

            // OSC 9 / OSC 777 desktop notification (Task 76).  Collect into
            // the out-parameter; the caller in `app_impl::update()` (where
            // `self.config` and the toast stack are in scope) dispatches each
            // request through the `NotificationRouter`.
            WindowManipulation::Notification {
                kind,
                source,
                title,
                body,
            } => {
                notifications.push(NotificationRequest {
                    kind,
                    source: Some(source),
                    title,
                    body,
                });
            }
            // OSC 99 stateful notification (Task 99.5a). Collect for
            // post-loop routing in `app_impl::update()`, where `self.config`,
            // the toast stack, and the OSC 99 session maps on `FreminalGui`
            // are all borrowable. A `Weak` handle to the pane's reply sender
            // travels alongside the data so the post-loop router (and the
            // long-lived desktop-notification thread) can send reverse
            // reports back to the originating pane WITHOUT keeping its PTY
            // consumer alive: a strong `Sender` clone would hold the input
            // channel open after the pane closed.
            WindowManipulation::Notification99(data) => {
                osc99_requests.push(Osc99Request::Display(Box::new(Osc99DisplayRequest {
                    data: *data,
                    reply_tx: Arc::downgrade(reply_tx),
                    origin: flags.osc99.origin,
                    pane_state: flags.osc99.pane_state,
                })));
            }
            // OSC 99 app→terminal control sequence (Task 99.5c). Collected
            // alongside a `Weak` reply handle for the originating pane and
            // answered after the drain loop, in order with the frame's other
            // OSC 99 requests.
            WindowManipulation::Osc99Control { id, kind } => {
                osc99_requests.push(Osc99Request::Control(Osc99ControlRequest {
                    control: Osc99Control { id, kind },
                    reply_tx: Arc::downgrade(reply_tx),
                    origin: flags.osc99.origin,
                }));
            }
            // OSC 99 icon priming (`g=` with icon bytes and no text). Nothing
            // is displayed or answered; the caller stores the bytes in the
            // session's icon cache when it reaches this request in the
            // frame's ordered list, so a display after it that names the key
            // finds them.
            WindowManipulation::Osc99CacheIcon { key, data } => {
                osc99_requests.push(Osc99Request::PrimeIcon(Osc99IconPrime { key, data }));
            }
        }
    }
    shell_set_title
}

#[cfg(test)]
mod osc52_toast_text_tests {
    use super::{Osc52ToastEvent, osc52_toast_text};

    #[test]
    fn wrote_maps_to_clipboard_updated_with_byte_count() {
        let (title, detail) = osc52_toast_text(&Osc52ToastEvent::Wrote { bytes: 5 });
        assert_eq!(title, "Clipboard updated");
        let detail = detail.expect("write event should carry a detail message");
        assert!(
            detail.contains('5'),
            "detail should mention the byte count: {detail}"
        );
        // Privacy: the detail must never contain clipboard content, only the
        // byte count. This test can't prove a negative for arbitrary content,
        // but it does assert the message is the fixed template, not raw text.
        assert_eq!(detail, "An application copied 5 bytes to the clipboard.");
    }

    #[test]
    fn read_blocked_maps_to_blocked_title() {
        let (title, detail) = osc52_toast_text(&Osc52ToastEvent::ReadBlocked);
        assert_eq!(title, "Clipboard read blocked");
        assert!(detail.is_some());
    }
}

#[cfg(test)]
mod window_manipulation_reply_tests {
    use super::{Osc52ToastEvent, WindowManipFlags, handle_window_manipulation};
    use crate::gui::frame_drain::WindowFocus;
    use crate::gui::osc99::{
        Osc99ControlRequest, Osc99DisplayRequest, Osc99IconCache, Osc99IconPrime, Osc99Origin,
        Osc99PaneContext, Osc99PaneState, Osc99Request, PaneActivity, PaneKeyboardFocus,
        PaneVisibility, TabActivity, WindowMinimized, ZoomOcclusion,
    };
    use crate::gui::panes::PaneIdGenerator;
    use crate::gui::title_stack::{MAX_TITLE_STACK_DEPTH, TitleStack};
    use freminal_windowing::WindowId;
    use std::sync::{Arc, Weak};

    use crossbeam_channel::{Receiver, Sender, unbounded};
    use freminal_common::buffer_states::osc99_notification::{
        Notification99Data, NotificationOccasion, NotificationUrgency, Osc99ActivationFocus,
        Osc99ActivationReporting, Osc99CloseReporting, Osc99ControlKind, Osc99Expiry, Osc99Sound,
    };
    use freminal_common::buffer_states::window_manipulation::WindowManipulation;
    use freminal_common::config::BellMode;
    use freminal_terminal_emulator::io::{GuiReply, InputEvent, WindowCommand, WindowStateReport};

    /// Everything [`handle_window_manipulation`] produced for one frame, plus
    /// the pane's reply handle so a test controls its lifetime.
    struct Drained {
        replies: Vec<GuiReply>,
        osc52_events: Vec<Osc52ToastEvent>,
        /// Every OSC 99 request, in arrival order.
        osc99: Vec<Osc99Request>,
        /// The pane's only strong sender; dropping it models the pane closing.
        pane_handle: Arc<Sender<InputEvent>>,
    }

    impl Drained {
        /// The display requests, in arrival order.
        fn displays(&self) -> Vec<&Osc99DisplayRequest> {
            self.osc99
                .iter()
                .filter_map(|request| match request {
                    Osc99Request::Display(display) => Some(display.as_ref()),
                    Osc99Request::Control(_) | Osc99Request::PrimeIcon(_) => None,
                })
                .collect()
        }

        /// The control requests, in arrival order.
        fn controls(&self) -> Vec<&Osc99ControlRequest> {
            self.osc99
                .iter()
                .filter_map(|request| match request {
                    Osc99Request::Control(control) => Some(control),
                    Osc99Request::Display(_) | Osc99Request::PrimeIcon(_) => None,
                })
                .collect()
        }

        /// The icon primings, in arrival order.
        fn primes(&self) -> Vec<&Osc99IconPrime> {
            self.osc99
                .iter()
                .filter_map(|request| match request {
                    Osc99Request::PrimeIcon(prime) => Some(prime),
                    Osc99Request::Display(_) | Osc99Request::Control(_) => None,
                })
                .collect()
        }
    }

    /// Run [`handle_window_manipulation`] once over `commands` inside a real
    /// (headless) egui frame.
    ///
    /// Clipboard reads are denied (`allow_clipboard_read: false`, the default),
    /// so no test depends on the host clipboard.
    fn drain_commands(commands: Vec<WindowManipulation>) -> Drained {
        drain_commands_with(commands, test_osc99_context())
    }

    /// The OSC 99 context the plain [`drain_commands`] uses: window 1, the
    /// first pane, focused and visible.
    fn test_osc99_context() -> Osc99PaneContext {
        let mut ids = PaneIdGenerator::new(0);
        Osc99PaneContext {
            origin: Osc99Origin {
                window_id: WindowId::for_test(1),
                pane_id: ids.next_id(),
            },
            pane_state: Osc99PaneState::compute(
                WindowFocus::Focused,
                WindowMinimized::Restored,
                TabActivity::Active,
                PaneActivity::Active,
                ZoomOcclusion::NotOccluded,
            ),
        }
    }

    fn drain_commands_with(commands: Vec<WindowManipulation>, osc99: Osc99PaneContext) -> Drained {
        let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();
        let (reply_tx, reply_rx): (_, Receiver<InputEvent>) = unbounded();
        let reply_tx = Arc::new(reply_tx);
        for cmd in commands {
            if let Err(e) = window_cmd_tx.send(WindowCommand::Report(cmd)) {
                panic!("send window command: {e}");
            }
        }

        let flags = WindowManipFlags {
            allow_clipboard_read: false,
            is_active: true,
            window_focused: true,
            is_only_pane: true,
            osc99,
        };
        let mut title_stack = TitleStack::new();
        let mut tab_title = String::new();
        let mut bell_active = false;
        let mut bell_since = None;
        let mut notifications = Vec::new();
        let mut osc99_requests = Vec::new();
        let mut osc52_events = Vec::new();

        let ctx = egui::Context::default();
        // No painter here, so the egui `TexturesDelta` drop-bomb must be
        // defused explicitly -- see A2 in EGUI_UPGRADE_ASSUMPTIONS.md.
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            handle_window_manipulation(
                ui,
                &window_cmd_rx,
                &reply_tx,
                8,
                16,
                ui.max_rect(),
                &mut title_stack,
                &mut tab_title,
                &mut bell_active,
                &mut bell_since,
                BellMode::None,
                &flags,
                &mut notifications,
                &mut osc99_requests,
                &mut osc52_events,
            );
        });
        full_output.textures_delta.clear();

        let replies = reply_rx
            .try_iter()
            .map(|event| match event {
                InputEvent::Reply(reply) => reply,
                other => panic!("expected InputEvent::Reply, got {other:?}"),
            })
            .collect();
        Drained {
            replies,
            osc52_events,
            osc99: osc99_requests,
            pane_handle: reply_tx,
        }
    }

    /// Whether the pane under test is the fully-active pane or a background one.
    #[derive(Clone, Copy)]
    enum PaneRole {
        Active,
        Background,
    }

    /// Run one frame of `commands` through `handle_window_manipulation` for a
    /// pane of the given `role`, mutating the caller's `title_stack` and
    /// `tab_title` so state persists across frames.
    fn run_title_frame(
        commands: Vec<WindowManipulation>,
        role: PaneRole,
        title_stack: &mut TitleStack,
        tab_title: &mut String,
    ) {
        let (window_cmd_tx, window_cmd_rx) = unbounded::<WindowCommand>();
        let (reply_tx, _reply_rx): (_, Receiver<InputEvent>) = unbounded();
        let reply_tx = Arc::new(reply_tx);
        for cmd in commands {
            if let Err(e) = window_cmd_tx.send(WindowCommand::Viewport(cmd)) {
                panic!("send window command: {e}");
            }
        }
        let flags = WindowManipFlags {
            allow_clipboard_read: false,
            is_active: matches!(role, PaneRole::Active),
            window_focused: true,
            is_only_pane: true,
            osc99: test_osc99_context(),
        };
        let mut bell_active = false;
        let mut bell_since = None;
        let mut notifications = Vec::new();
        let mut osc99_requests = Vec::new();
        let mut osc52_events = Vec::new();

        let ctx = egui::Context::default();
        // No painter here: defuse the `TexturesDelta` drop-bomb (A2 in
        // EGUI_UPGRADE_ASSUMPTIONS.md).
        let mut full_output = ctx.run_ui(egui::RawInput::default(), |ui| {
            handle_window_manipulation(
                ui,
                &window_cmd_rx,
                &reply_tx,
                8,
                16,
                ui.max_rect(),
                title_stack,
                tab_title,
                &mut bell_active,
                &mut bell_since,
                BellMode::None,
                &flags,
                &mut notifications,
                &mut osc99_requests,
                &mut osc52_events,
            );
        });
        full_output.textures_delta.clear();
    }

    #[test]
    fn background_pane_title_stack_is_bounded_and_restores_newest_first() {
        let mut title_stack = TitleStack::new();
        let mut tab_title = String::new();
        // A background pane saves its own tab title; vary it per push so the
        // eviction order is observable.
        let mut commands = Vec::new();
        for i in 0..5000 {
            commands.push(WindowManipulation::SetTitleBarText(format!("t{i}")));
            commands.push(WindowManipulation::SaveWindowTitleToStack);
        }
        run_title_frame(
            commands,
            PaneRole::Background,
            &mut title_stack,
            &mut tab_title,
        );
        assert_eq!(title_stack.len(), MAX_TITLE_STACK_DEPTH);

        for i in (4990..5000).rev() {
            run_title_frame(
                vec![WindowManipulation::RestoreWindowTitleFromStack],
                PaneRole::Background,
                &mut title_stack,
                &mut tab_title,
            );
            assert_eq!(tab_title, format!("t{i}"));
        }
        assert!(title_stack.is_empty());
        run_title_frame(
            vec![WindowManipulation::RestoreWindowTitleFromStack],
            PaneRole::Background,
            &mut title_stack,
            &mut tab_title,
        );
        assert_eq!(tab_title, "", "an empty stack clears the title");
    }

    #[test]
    fn active_pane_title_stack_is_bounded_and_restores_newest_first() {
        let mut title_stack = TitleStack::new();
        let mut tab_title = String::new();
        // The active pane saves the viewport title, which cannot change within
        // a frame; seed 10 distinct, observable entries first via the
        // pre-populated stack, then overflow it.
        for i in 0..MAX_TITLE_STACK_DEPTH {
            title_stack.push(format!("old{i}"));
        }
        let commands = vec![WindowManipulation::SaveWindowTitleToStack; 5000];
        run_title_frame(commands, PaneRole::Active, &mut title_stack, &mut tab_title);
        assert_eq!(title_stack.len(), MAX_TITLE_STACK_DEPTH);

        // Every pre-seeded entry was evicted by the newer saves.
        for _ in 0..MAX_TITLE_STACK_DEPTH {
            run_title_frame(
                vec![WindowManipulation::RestoreWindowTitleFromStack],
                PaneRole::Active,
                &mut title_stack,
                &mut tab_title,
            );
            assert_eq!(tab_title, "Freminal");
        }
        assert!(title_stack.is_empty());
    }

    /// The replies and OSC 52 toast events from one frame.
    fn run_commands(commands: Vec<WindowManipulation>) -> (Vec<GuiReply>, Vec<Osc52ToastEvent>) {
        let drained = drain_commands(commands);
        (drained.replies, drained.osc52_events)
    }

    fn sample_notification() -> Notification99Data {
        Notification99Data {
            id: Some("n1".to_owned()),
            title: "Title".to_owned(),
            body: None,
            icon_data: None,
            icon_names: Vec::new(),
            icon_cache_key: None,
            buttons: Vec::new(),
            activation_reporting: Osc99ActivationReporting::Silent,
            activation_focus: Osc99ActivationFocus::Focus,
            close_reporting: Osc99CloseReporting::Silent,
            urgency: NotificationUrgency::Normal,
            occasion: NotificationOccasion::Always,
            sound: Osc99Sound::System,
            app_name: None,
            notification_types: Vec::new(),
            expiry: Osc99Expiry::OsDefault,
        }
    }

    #[test]
    fn osc99_requests_carry_the_panes_origin_and_state() {
        let mut ids = PaneIdGenerator::new(7);
        let osc99 = Osc99PaneContext {
            origin: Osc99Origin {
                window_id: WindowId::for_test(42),
                pane_id: ids.next_id(),
            },
            pane_state: Osc99PaneState {
                focus: PaneKeyboardFocus::Unfocused,
                visibility: PaneVisibility::Hidden,
            },
        };
        let drained = drain_commands_with(
            vec![
                WindowManipulation::Notification99(Box::new(sample_notification())),
                WindowManipulation::Osc99Control {
                    id: "n1".to_owned(),
                    kind: Osc99ControlKind::Close,
                },
            ],
            osc99,
        );

        let (displays, controls) = (drained.displays(), drained.controls());
        assert_eq!(displays.len(), 1);
        assert_eq!(displays[0].origin, osc99.origin);
        assert_eq!(displays[0].pane_state, osc99.pane_state);
        assert_eq!(controls.len(), 1);
        assert_eq!(controls[0].origin, osc99.origin);
    }

    #[test]
    fn osc99_requests_of_every_kind_are_collected_in_arrival_order() {
        let drained = drain_commands(vec![
            WindowManipulation::Osc99CacheIcon {
                key: "k".to_owned(),
                data: vec![1],
            },
            WindowManipulation::Notification99(Box::new(sample_notification())),
            WindowManipulation::Osc99Control {
                id: "n1".to_owned(),
                kind: Osc99ControlKind::Close,
            },
            WindowManipulation::Notification99(Box::new(sample_notification())),
            WindowManipulation::Osc99Control {
                id: "n1".to_owned(),
                kind: Osc99ControlKind::Alive,
            },
        ]);

        let shape: Vec<&str> = drained
            .osc99
            .iter()
            .map(|request| match request {
                Osc99Request::Display(_) => "display",
                Osc99Request::Control(_) => "control",
                Osc99Request::PrimeIcon(_) => "prime",
            })
            .collect();
        assert_eq!(
            shape,
            ["prime", "display", "control", "display", "control"],
            "one ordered list: the order between kinds is the order the pane emitted them"
        );
    }

    #[test]
    fn osc99_cache_icon_is_collected_and_reaches_the_icon_cache() {
        let drained = drain_commands(vec![
            WindowManipulation::Osc99CacheIcon {
                key: "k".to_owned(),
                data: vec![1, 2, 3],
            },
            // A later priming under the same key replaces the earlier bytes.
            WindowManipulation::Osc99CacheIcon {
                key: "other".to_owned(),
                data: vec![9],
            },
            WindowManipulation::Osc99CacheIcon {
                key: "k".to_owned(),
                data: vec![4, 5],
            },
        ]);
        assert_eq!(
            drained.primes(),
            [
                &Osc99IconPrime {
                    key: "k".to_owned(),
                    data: vec![1, 2, 3]
                },
                &Osc99IconPrime {
                    key: "other".to_owned(),
                    data: vec![9]
                },
                &Osc99IconPrime {
                    key: "k".to_owned(),
                    data: vec![4, 5]
                },
            ],
            "priming is collected in arrival order"
        );
        assert!(drained.displays().is_empty());
        assert!(drained.controls().is_empty());
        assert!(drained.replies.is_empty(), "priming is never answered");

        let mut cache = Osc99IconCache::default();
        for prime in drained.primes() {
            prime.clone().store_in(&mut cache);
        }
        assert_eq!(cache.get("k"), Some([4u8, 5].as_slice()));
        assert_eq!(cache.get("other"), Some([9u8].as_slice()));
        assert_eq!(cache.get("missing"), None);
    }

    #[test]
    fn osc99_handles_stop_upgrading_once_the_pane_handle_drops() {
        let drained = drain_commands(vec![
            WindowManipulation::Notification99(Box::new(sample_notification())),
            WindowManipulation::Osc99Control {
                id: "n1".to_owned(),
                kind: Osc99ControlKind::Alive,
            },
        ]);
        assert_eq!(drained.displays().len(), 1);
        assert_eq!(drained.controls().len(), 1);
        assert!(
            drained.replies.is_empty(),
            "OSC 99 commands are routed, not answered, here"
        );

        let notification_handle = Weak::clone(&drained.displays()[0].reply_tx);
        let control_handle = Weak::clone(&drained.controls()[0].reply_tx);

        // While the pane lives, the collected handles reach its input channel.
        assert!(notification_handle.upgrade().is_some());
        assert!(control_handle.upgrade().is_some());

        // The pane closing drops its only strong handle: a collected handle
        // must not keep the PTY consumer's channel open.
        let Drained { pane_handle, .. } = drained;
        drop(pane_handle);
        assert!(notification_handle.upgrade().is_none());
        assert!(control_handle.upgrade().is_none());
    }

    #[test]
    fn report_title_replies_through_input_channel() {
        let (replies, _) = run_commands(vec![WindowManipulation::ReportTitle]);
        // A default egui context has no viewport title, so the handler falls
        // back to "Freminal".
        assert_eq!(replies, vec![GuiReply::WindowTitle("Freminal".to_owned())]);
    }

    #[test]
    fn report_icon_label_replies_through_input_channel() {
        let (replies, _) = run_commands(vec![WindowManipulation::ReportIconLabel]);
        assert_eq!(replies, vec![GuiReply::IconLabel("Freminal".to_owned())]);
    }

    #[test]
    fn report_window_state_replies_normal_when_not_minimized() {
        let (replies, _) = run_commands(vec![WindowManipulation::ReportWindowState]);
        assert_eq!(
            replies,
            vec![GuiReply::WindowState(WindowStateReport::Normal)]
        );
    }

    #[test]
    fn query_clipboard_denied_replies_with_empty_payload_and_toast_event() {
        let (replies, events) =
            run_commands(vec![WindowManipulation::QueryClipboard("c".to_owned())]);
        assert_eq!(
            replies,
            vec![GuiReply::Clipboard {
                selection: "c".to_owned(),
                base64_payload: String::new(),
            }]
        );
        assert!(
            matches!(events.as_slice(), [Osc52ToastEvent::ReadBlocked]),
            "a denied read must still surface the blocked toast: {events:?}"
        );
    }

    #[test]
    fn replies_arrive_in_command_order() {
        let (replies, _) = run_commands(vec![
            WindowManipulation::ReportWindowState,
            WindowManipulation::ReportTitle,
        ]);
        assert_eq!(
            replies,
            vec![
                GuiReply::WindowState(WindowStateReport::Normal),
                GuiReply::WindowTitle("Freminal".to_owned()),
            ]
        );
    }
}
