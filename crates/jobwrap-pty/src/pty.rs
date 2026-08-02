//! Pseudo-terminal allocation and management.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};

use crate::ffi;
use crate::terminal::{SavedTerminal, SlaveTermios};
use jobwrap_core::WindowSize;

/// A pseudo-terminal pair.
#[derive(Debug)]
pub struct PseudoTerminal {
    master: OwnedFd,
    slave: Option<OwnedFd>,
}

impl PseudoTerminal {
    /// Allocate a new PTY.
    pub fn allocate() -> io::Result<Self> {
        let (master, slave) = ffi::openpty()?;
        Ok(Self {
            master,
            slave: Some(slave),
        })
    }

    /// The master descriptor.
    pub fn master_fd(&self) -> i32 {
        self.master.as_raw_fd()
    }

    /// The slave descriptor.
    pub fn slave_fd(&self) -> i32 {
        self.slave.as_ref().map(|f| f.as_raw_fd()).unwrap_or(-1)
    }

    /// Take ownership of the slave descriptor (used by the child). The parent
    /// keeps the master so the relay can continue.
    pub fn take_slave(&mut self) -> Option<OwnedFd> {
        self.slave.take()
    }

    /// The current window size of the PTY.
    pub fn window_size(&self) -> io::Result<WindowSize> {
        ffi::get_window_size(self.master_fd())
    }

    /// Set the window size of the PTY.
    pub fn set_window_size(&self, ws: WindowSize) -> io::Result<()> {
        ffi::set_window_size(self.master_fd(), &ws)
    }

    /// Initialize the slave from a captured parent terminal.
    pub fn initialize_slave(&self, parent: Option<&SavedTerminal>) -> io::Result<()> {
        let termios = SlaveTermios::from_parent(parent);
        termios.apply(self.slave_fd())
    }
}

impl AsRawFd for PseudoTerminal {
    fn as_raw_fd(&self) -> i32 {
        self.master_fd()
    }
}
