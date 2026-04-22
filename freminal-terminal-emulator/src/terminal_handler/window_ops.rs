// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

//! Window manipulation dispatch and command queue for [`TerminalHandler`].
//!
//! Handles `XTWINOPS` (CSI Ps t) window manipulation sequences and provides
//! `take_window_commands` for the GUI thread to consume deferred commands.

use freminal_common::buffer_states::window_manipulation::WindowManipulation;

use super::TerminalHandler;

impl TerminalHandler {
    /// Drain and return all queued `WindowManipulation` commands.
    pub fn take_window_commands(&mut self) -> Vec<WindowManipulation> {
        std::mem::take(&mut self.window_commands)
    }

    /// Handle a `WindowManipulation` command.
    ///
    /// Report variants that can be answered from terminal state are handled
    /// synchronously here via `write_to_pty` so the response reaches the PTY
    /// in the same processing batch as DA1 and other inline responses.  This
    /// is critical for applications (e.g. yazi) that use DA1 as a "fence" to
    /// detect when all prior query responses have arrived.
    ///
    /// Variants that require GUI-side data (viewport position, window title,
    /// clipboard, etc.) are deferred to `self.window_commands` for the GUI
    /// thread to handle asynchronously.
    pub(super) fn handle_window_manipulation(&mut self, wm: &WindowManipulation) {
        match wm {
            WindowManipulation::ReportCharacterSizeInPixels => {
                let w = self.cell_pixel_width;
                let h = self.cell_pixel_height;
                self.write_csi_response(&format!("6;{h};{w}t"));
            }
            WindowManipulation::ReportTerminalSizeInCharacters => {
                let (width, height) = self.win_size();
                self.write_csi_response(&format!("8;{height};{width}t"));
            }
            WindowManipulation::ReportRootWindowSizeInCharacters => {
                let (width, height) = self.win_size();
                self.write_csi_response(&format!("9;{height};{width}t"));
            }
            other => {
                self.window_commands.push(other.clone());
            }
        }
    }
}
