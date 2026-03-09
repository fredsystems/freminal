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
