// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

/// The pixel/character dimensions of the terminal window.
///
/// Used inside [`PtyWrite::Resize`] to inform the PTY of a window-size change.
#[derive(Debug, Clone)]
pub struct FreminalTerminalSize {
    pub width: usize,
    pub height: usize,
    pub pixel_width: usize,
    pub pixel_height: usize,
}

/// Commands sent from the terminal-emulator layer to the OS PTY writer thread.
///
/// `Write` carries raw bytes to be forwarded to the shell/program running in the
/// PTY.  `Resize` notifies the OS that the terminal window has been resized so
/// that the kernel can update the PTY's `winsize` struct and deliver `SIGWINCH`
/// to the child process.
#[derive(Debug)]
pub enum PtyWrite {
    /// Raw bytes to write to the PTY.
    Write(Vec<u8>),
    /// Resize the PTY to the given dimensions.
    Resize(FreminalTerminalSize),
}
