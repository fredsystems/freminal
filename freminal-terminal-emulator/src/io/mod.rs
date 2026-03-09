// Copyright (C) 2024-2026 Fred Clausen
// Use of this source code is governed by an MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT.

pub mod dummy;
pub use dummy::DummyIo;
mod pty;
use anyhow::{Error, Result};
use conv2::ValueFrom;
use portable_pty::PtySize;
pub use pty::FreminalPtyInputOutput;
// pub type TermIoErr = Box<dyn std::error::Error>;

// pub enum ReadResponse {
//     Success(usize),
//     Empty,
// }

#[derive(Debug)]
pub struct FreminalTerminalSize {
    pub width: usize,
    pub height: usize,
    pub pixel_width: usize,
    pub pixel_height: usize,
}

impl TryFrom<FreminalTerminalSize> for PtySize {
    type Error = Error;

    fn try_from(value: FreminalTerminalSize) -> Result<Self> {
        Ok(Self {
            rows: u16::value_from(value.height)?,
            cols: u16::value_from(value.width)?,
            pixel_width: u16::value_from(value.pixel_width)?,
            pixel_height: u16::value_from(value.pixel_height)?,
        })
    }
}

pub struct PtyRead {
    pub buf: Vec<u8>,
    pub read_amount: usize,
}

#[derive(Debug)]
pub enum PtyWrite {
    Write(Vec<u8>),
    Resize(FreminalTerminalSize),
}

pub trait FreminalTermInputOutput {
    // fn read(&mut self, buf: &mut [u8]);
    // fn write(&mut self, buf: &[u8]);
    // fn set_win_size(&mut self, width: usize, height: usize);
}

/// Events sent from the GUI thread to the PTY processing thread.
///
/// The GUI sends these through a `crossbeam_channel::Sender<InputEvent>`.
/// The PTY thread receives them in its `select!` loop alongside `PtyRead`.
///
/// No call sites are wired up yet — this is a type definition only.
#[derive(Debug, Clone)]
pub enum InputEvent {
    /// Raw bytes to write to the PTY (keyboard input).
    Key(Vec<u8>),
    /// New terminal dimensions in character cells `(width, height)`.
    Resize(usize, usize),
    /// Window focus gained (`true`) or lost (`false`).
    FocusChange(bool),
}

/// Commands sent from the PTY processing thread to the GUI thread.
///
/// The PTY thread sends these through a `crossbeam_channel::Sender<WindowCommand>`
/// when the terminal program requests window-level operations.  The GUI drains
/// them at the start of each `update()` call via non-blocking `try_recv()`.
///
/// No call sites are wired up yet — this is a type definition only.
#[derive(Debug, Clone)]
pub enum WindowCommand {
    /// A viewport-level command that the GUI should execute via
    /// `ctx.send_viewport_cmd(…)`.
    Viewport(freminal_common::buffer_states::window_manipulation::WindowManipulation),
    /// A query that requires the GUI to read viewport geometry and write a
    /// response back to the PTY via its `Sender<PtyWrite>`.
    Report(freminal_common::buffer_states::window_manipulation::WindowManipulation),
}
