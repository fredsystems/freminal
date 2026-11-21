use crate::cmdbuilder::CommandBuilder;
use crate::win::pseudocon::PseudoCon;
use crate::{Child, MasterPty, PtyPair, PtySize, PtySystem, SlavePty};
use anyhow::Error;
use filedescriptor::{FileDescriptor, Pipe};
use std::sync::{Arc, Mutex};
use winapi::um::wincon::COORD;

/// A readable `ConPTY` handle that also satisfies [`crate::PtyReader`].
///
/// Windows `ConPTY` exposes no termios, so `PtyReader` on this platform is just
/// `Read + Send` with no extra methods — this wrapper merely forwards reads.
struct WinPtyReader {
    inner: FileDescriptor,
}

impl std::io::Read for WinPtyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buf)
    }
}

impl crate::PtyReader for WinPtyReader {}

#[derive(Default)]
pub struct ConPtySystem {}

impl PtySystem for ConPtySystem {
    fn openpty(&self, size: PtySize) -> anyhow::Result<PtyPair> {
        let stdin = Pipe::new()?;
        let stdout = Pipe::new()?;

        let con = PseudoCon::new(
            console_coord(size.cols, size.rows)?,
            stdin.read,
            stdout.write,
        )?;

        let master = ConPtyMasterPty {
            inner: Arc::new(Mutex::new(Inner {
                con,
                readable: stdout.read,
                writable: Some(stdin.write),
                size,
            })),
        };

        let slave = ConPtySlavePty {
            inner: master.inner.clone(),
        };

        Ok(PtyPair {
            master: Box::new(master),
            slave: Box::new(slave),
        })
    }
}

struct Inner {
    con: PseudoCon,
    readable: FileDescriptor,
    writable: Option<FileDescriptor>,
    size: PtySize,
}

impl Inner {
    pub fn resize(
        &mut self,
        num_rows: u16,
        num_cols: u16,
        pixel_width: u16,
        pixel_height: u16,
    ) -> Result<(), Error> {
        self.con.resize(console_coord(num_cols, num_rows)?)?;
        self.size = PtySize {
            rows: num_rows,
            cols: num_cols,
            pixel_width,
            pixel_height,
        };
        Ok(())
    }
}

/// Convert a pty size to a console `COORD`, rejecting sizes beyond `i16::MAX`
/// rather than wrapping them into a negative size.
fn console_coord(cols: u16, rows: u16) -> anyhow::Result<COORD> {
    Ok(COORD {
        X: i16::try_from(cols).map_err(|e| anyhow::anyhow!("pty width {cols} too large: {e}"))?,
        Y: i16::try_from(rows).map_err(|e| anyhow::anyhow!("pty height {rows} too large: {e}"))?,
    })
}

/// Helper to lock the inner mutex, mapping a poisoned mutex to an anyhow error.
fn lock_inner(inner: &Mutex<Inner>) -> anyhow::Result<std::sync::MutexGuard<'_, Inner>> {
    inner
        .lock()
        .map_err(|e| anyhow::anyhow!("pty inner mutex poisoned: {e}"))
}

#[derive(Clone)]
pub struct ConPtyMasterPty {
    inner: Arc<Mutex<Inner>>,
}

pub struct ConPtySlavePty {
    inner: Arc<Mutex<Inner>>,
}

impl MasterPty for ConPtyMasterPty {
    fn resize(&self, size: PtySize) -> anyhow::Result<()> {
        let mut inner = lock_inner(&self.inner)?;
        inner.resize(size.rows, size.cols, size.pixel_width, size.pixel_height)
    }

    fn get_size(&self) -> Result<PtySize, Error> {
        let inner = lock_inner(&self.inner)?;
        Ok(inner.size)
    }

    fn try_clone_reader(&self) -> anyhow::Result<Box<dyn std::io::Read + Send>> {
        Ok(Box::new(lock_inner(&self.inner)?.readable.try_clone()?))
    }

    fn try_clone_reader_termios(&self) -> anyhow::Result<Box<dyn crate::PtyReader>> {
        // Windows ConPTY has no termios; the reader is a plain readable handle.
        // Wrap it so it satisfies `PtyReader` (which, on non-unix, is just
        // `Read + Send` with no extra methods).
        Ok(Box::new(WinPtyReader {
            inner: lock_inner(&self.inner)?.readable.try_clone()?,
        }))
    }

    fn take_writer(&self) -> anyhow::Result<Box<dyn std::io::Write + Send>> {
        Ok(Box::new(
            lock_inner(&self.inner)?
                .writable
                .take()
                .ok_or_else(|| anyhow::anyhow!("writer already taken"))?,
        ))
    }
}

impl SlavePty for ConPtySlavePty {
    fn spawn_command(&self, cmd: CommandBuilder) -> anyhow::Result<Box<dyn Child + Send + Sync>> {
        let child = lock_inner(&self.inner)?.con.spawn_command(&cmd)?;
        Ok(Box::new(child))
    }
}
