use crate::{Child, ChildKiller, ExitStatus};
use anyhow::Context as _;
use std::io::{Error as IoError, Result as IoResult};
use std::os::windows::io::{AsRawHandle, RawHandle};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::task::{Context, Poll};
use winapi::shared::minwindef::DWORD;
use winapi::um::minwinbase::STILL_ACTIVE;
use winapi::um::processthreadsapi::*;
use winapi::um::synchapi::WaitForSingleObject;
use winapi::um::winbase::INFINITE;

pub mod conpty;
mod procthreadattr;
mod pseudocon;

use filedescriptor::OwnedHandle;

#[derive(Debug)]
pub struct WinChild {
    proc: Mutex<OwnedHandle>,
    /// Tracks whether a waiter thread has already been spawned for `Future::poll`.
    waiter_spawned: AtomicBool,
}

/// Helper to lock the process handle mutex, converting a poisoned-mutex error
/// to `std::io::Error`.
fn lock_proc(proc: &Mutex<OwnedHandle>) -> IoResult<std::sync::MutexGuard<'_, OwnedHandle>> {
    proc.lock().map_err(|e| {
        IoError::new(
            std::io::ErrorKind::Other,
            format!("process handle mutex poisoned: {e}"),
        )
    })
}

impl WinChild {
    fn is_complete(&mut self) -> IoResult<Option<ExitStatus>> {
        let mut status: DWORD = 0;
        let proc = lock_proc(&self.proc)?.try_clone().map_err(|e| {
            IoError::new(
                std::io::ErrorKind::Other,
                format!("failed to clone process handle: {e}"),
            )
        })?;
        let res = unsafe { GetExitCodeProcess(proc.as_raw_handle() as _, &mut status) };
        if res != 0 {
            if status == STILL_ACTIVE {
                Ok(None)
            } else {
                Ok(Some(ExitStatus::with_exit_code(status)))
            }
        } else {
            Ok(None)
        }
    }

    fn do_kill(&mut self) -> IoResult<()> {
        let proc = lock_proc(&self.proc)?.try_clone().map_err(|e| {
            IoError::new(
                std::io::ErrorKind::Other,
                format!("failed to clone process handle: {e}"),
            )
        })?;
        let res = unsafe { TerminateProcess(proc.as_raw_handle() as _, 1) };
        let err = IoError::last_os_error();
        if res == 0 {
            Err(err)
        } else {
            Ok(())
        }
    }
}

impl ChildKiller for WinChild {
    fn kill(&mut self) -> IoResult<()> {
        self.do_kill()
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        // Best-effort: if the lock is poisoned or the clone fails, return a
        // no-op killer rather than panicking.
        let proc_handle = lock_proc(&self.proc)
            .ok()
            .and_then(|guard| guard.try_clone().ok());
        match proc_handle {
            Some(handle) => Box::new(WinChildKiller { proc: Some(handle) }),
            None => Box::new(WinChildKiller { proc: None }),
        }
    }
}

#[derive(Debug)]
pub struct WinChildKiller {
    proc: Option<OwnedHandle>,
}

impl ChildKiller for WinChildKiller {
    fn kill(&mut self) -> IoResult<()> {
        let Some(proc) = &self.proc else {
            return Err(IoError::new(
                std::io::ErrorKind::Other,
                "no process handle available to kill",
            ));
        };
        let res = unsafe { TerminateProcess(proc.as_raw_handle() as _, 1) };
        let err = IoError::last_os_error();
        if res == 0 {
            Err(err)
        } else {
            Ok(())
        }
    }

    fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
        let handle = self.proc.as_ref().and_then(|h| h.try_clone().ok());
        Box::new(WinChildKiller { proc: handle })
    }
}

impl Child for WinChild {
    fn try_wait(&mut self) -> IoResult<Option<ExitStatus>> {
        self.is_complete()
    }

    fn wait(&mut self) -> IoResult<ExitStatus> {
        if let Ok(Some(status)) = self.try_wait() {
            return Ok(status);
        }
        let proc = lock_proc(&self.proc)?.try_clone().map_err(|e| {
            IoError::new(
                std::io::ErrorKind::Other,
                format!("failed to clone process handle: {e}"),
            )
        })?;
        unsafe {
            WaitForSingleObject(proc.as_raw_handle() as _, INFINITE);
        }
        let mut status: DWORD = 0;
        let res = unsafe { GetExitCodeProcess(proc.as_raw_handle() as _, &mut status) };
        if res != 0 {
            Ok(ExitStatus::with_exit_code(status))
        } else {
            Err(IoError::last_os_error())
        }
    }

    fn process_id(&self) -> Option<u32> {
        let guard = lock_proc(&self.proc).ok()?;
        let res = unsafe { GetProcessId(guard.as_raw_handle() as _) };
        if res == 0 {
            None
        } else {
            Some(res)
        }
    }

    fn as_raw_handle(&self) -> Option<std::os::windows::io::RawHandle> {
        let proc = lock_proc(&self.proc).ok()?;
        Some(proc.as_raw_handle())
    }
}

impl std::future::Future for WinChild {
    type Output = anyhow::Result<ExitStatus>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<anyhow::Result<ExitStatus>> {
        match self.is_complete() {
            Ok(Some(status)) => Poll::Ready(Ok(status)),
            Err(err) => Poll::Ready(Err(err).context("Failed to retrieve process exit status")),
            Ok(None) => {
                // Only spawn a single waiter thread.  Previous code spawned a new
                // OS thread on every `poll`, which is wasteful and could leak
                // threads if the future is polled frequently before completion.
                if self
                    .waiter_spawned
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    struct PassRawHandleToWaiterThread(pub RawHandle);
                    // SAFETY: Windows HANDLEs are plain pointer-sized values that
                    // are safe to send between threads.  The handle is kept alive
                    // by the `Mutex<OwnedHandle>` in `WinChild` which outlives the
                    // waiter thread (the Future cannot be dropped while being
                    // polled).
                    unsafe impl Send for PassRawHandleToWaiterThread {}

                    let proc = match lock_proc(&self.proc)
                        .map_err(anyhow::Error::from)
                        .and_then(|g| g.try_clone().map_err(anyhow::Error::from))
                    {
                        Ok(p) => p,
                        Err(e) => return Poll::Ready(Err(e)),
                    };
                    let handle = PassRawHandleToWaiterThread(proc.as_raw_handle());

                    let waker = cx.waker().clone();
                    std::thread::spawn(move || {
                        unsafe {
                            WaitForSingleObject(handle.0 as _, INFINITE);
                        }
                        waker.wake();
                    });
                }
                Poll::Pending
            }
        }
    }
}
