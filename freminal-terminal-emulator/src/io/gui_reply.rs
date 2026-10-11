// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! GUI-originated terminal-to-application replies.
//!
//! Some replies to an application's query can only be produced by the GUI
//! thread: it alone knows the window position and size, the window title, the
//! system clipboard, and the fate of an OSC 99 notification. The GUI sends them
//! to the PTY thread as structured data ([`GuiReply`], carried by
//! [`InputEvent::Reply`](super::InputEvent::Reply)) rather than as pre-formatted
//! escape-sequence bytes, so the PTY thread frames them itself and honours the
//! application's S8C1T mode.
//!
//! The bodies the serialiser produces are byte-identical to the GUI's former
//! hand-formatted strings, minus their 7-bit framing.

/// Whether the window is minimized, as reported in answer to the `CSI 11 t`
/// window-state query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowStateReport {
    /// The window is not minimized. Serialised as `CSI 1 t`.
    Normal,
    /// The window is minimized (iconified). Serialised as `CSI 2 t`.
    Iconified,
}

/// Whether a closed OSC 99 notification's close was directly observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Osc99CloseTracking {
    /// The close was directly observed. The report payload is empty.
    Tracked,
    /// The close could not be directly observed (platforms whose background
    /// thread cannot watch for a close event). The report payload is
    /// `untracked`.
    Untracked,
}

/// A reply that only the GUI thread can produce, destined for the application.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiReply {
    /// Window state report: `CSI 1 t` (normal) or `CSI 2 t` (iconified).
    WindowState(WindowStateReport),
    /// Window position report: `CSI 3 ; x ; y t`.
    WindowPosition {
        /// Horizontal position in pixels.
        x: usize,
        /// Vertical position in pixels.
        y: usize,
    },
    /// Window size report in pixels: `CSI 4 ; height ; width t`.
    WindowSizePixels {
        /// Height in pixels.
        height: usize,
        /// Width in pixels.
        width: usize,
    },
    /// Screen (root window) size report in pixels: `CSI 5 ; height ; width t`.
    ScreenSizePixels {
        /// Height in pixels.
        height: usize,
        /// Width in pixels.
        width: usize,
    },
    /// Icon label report: `OSC L <label> ST`.
    IconLabel(String),
    /// Window title report: `OSC l <title> ST`.
    WindowTitle(String),
    /// OSC 52 clipboard query answer: `OSC 52 ; <selection> ; <payload> ST`.
    Clipboard {
        /// The selection parameter, echoed from the query (for example `c`).
        selection: String,
        /// The clipboard contents, base64-encoded; empty when the read was
        /// denied.
        base64_payload: String,
    },
    /// OSC 99 activation report: `OSC 99 ; i=<id> ; <button> ST`.
    Osc99Activation {
        /// The notification id; serialised as `0` when absent.
        id: Option<String>,
        /// The activated button; serialised as empty when absent (the whole
        /// notification was activated).
        button: Option<String>,
    },
    /// OSC 99 close report: `OSC 99 ; i=<id> : p=close ; [untracked] ST`.
    Osc99Closed {
        /// The notification id; serialised as `0` when absent.
        id: Option<String>,
        /// Whether the close was directly observed.
        tracking: Osc99CloseTracking,
    },
    /// OSC 99 alive report: `OSC 99 ; i=<id> : p=alive ; a,b ST`.
    Osc99Alive {
        /// The id of the `p=alive` request; serialised as `0` when absent.
        request_id: Option<String>,
        /// The ids of the notifications still live, comma-joined verbatim.
        live_ids: Vec<String>,
    },
}
