//! RAII guard that restores terminal state on every exit path.

use std::io;
use std::os::fd::RawFd;

use crate::ffi;
use jobwrap_core::{ProcessGroupId, WindowSize};

/// The state of the controlling terminal captured before the wrapped command
/// ran.
pub struct SavedTerminal {
    fd: RawFd,
    termios: Option<ffi::Termios>,
    foreground_pgid: Option<ProcessGroupId>,
    pub window_size: Option<WindowSize>,
    pub device: Option<String>,
    pub is_tty: bool,
}

impl std::fmt::Debug for SavedTerminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SavedTerminal")
            .field("fd", &self.fd)
            .field("foreground_pgid", &self.foreground_pgid)
            .field("window_size", &self.window_size)
            .field("device", &self.device)
            .field("is_tty", &self.is_tty)
            .finish()
    }
}

impl SavedTerminal {
    /// Capture the current terminal state of `fd`.
    pub fn capture(fd: RawFd) -> SavedTerminal {
        let termios = ffi::tcgetattr(fd).ok();
        let is_tty = termios.is_some();
        let foreground_pgid = ffi::tcgetpgrp(fd).ok();
        let window_size = ffi::get_window_size(fd).ok();
        let device = if is_tty { ffi::ttyname(fd) } else { None };
        SavedTerminal {
            fd,
            termios,
            foreground_pgid,
            window_size,
            device,
            is_tty,
        }
    }

    pub fn as_raw_fd(&self) -> RawFd {
        self.fd
    }

    /// Restore the captured terminal state. Best-effort.
    pub fn restore(&self) {
        if let Some(termios) = &self.termios {
            let _ = ffi::tcsetattr(self.fd, libc::TCSANOW, termios);
        }
        if let Some(pgid) = self.foreground_pgid {
            let _ = ffi::tcsetpgrp(self.fd, pgid);
        }
    }
}

/// The terminal state used to initialize the child's slave device.
pub struct SlaveTermios {
    termios: ffi::Termios,
}

impl std::fmt::Debug for SlaveTermios {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SlaveTermios").finish_non_exhaustive()
    }
}

impl SlaveTermios {
    /// Build the slave termios from the captured parent terminal, falling back
    /// to a sane canonical terminal when the parent was not a tty.
    pub fn from_parent(parent: Option<&SavedTerminal>) -> SlaveTermios {
        let termios = parent
            .and_then(|s| s.termios)
            .unwrap_or_else(ffi::default_termios);
        SlaveTermios { termios }
    }

    /// Apply to the slave descriptor before the child execs.
    pub fn apply(&self, fd: RawFd) -> io::Result<()> {
        ffi::tcsetattr(fd, libc::TCSANOW, &self.termios)
    }
}

/// RAII guard restoring a saved terminal when dropped.
pub struct TerminalGuard<'a> {
    saved: &'a SavedTerminal,
    armed: bool,
}

impl<'a> TerminalGuard<'a> {
    pub fn new(saved: &'a SavedTerminal) -> Self {
        Self { saved, armed: true }
    }

    pub fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TerminalGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.saved.restore();
        }
    }
}

/// RAII guard that applies relay (raw-ish) mode to the wrapper's terminal and
/// restores the captured state on drop.
pub struct RelayModeGuard {
    saved: SavedTerminal,
    armed: bool,
}

impl RelayModeGuard {
    /// Capture and switch `fd` into relay mode.
    pub fn apply(fd: RawFd) -> io::Result<Self> {
        let saved = SavedTerminal::capture(fd);
        if saved.is_tty {
            ffi::set_relay_mode(fd)?;
        }
        Ok(Self { saved, armed: true })
    }

    pub fn saved(&self) -> &SavedTerminal {
        &self.saved
    }
}

impl Drop for RelayModeGuard {
    fn drop(&mut self) {
        if self.armed {
            self.saved.restore();
        }
    }
}
