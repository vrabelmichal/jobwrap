//! Child process spawning and status reporting.

use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

use crate::ffi;
use jobwrap_core::{ProcessGroupId, ProcessId, SessionId};

/// Identifiers of the spawned child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpawnedChild {
    pub pid: ProcessId,
    pub process_group: ProcessGroupId,
    pub session: SessionId,
}

/// The result of waiting for a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStatus {
    /// The child exited with a code.
    Exited { code: i32 },
    /// The child was terminated by a signal.
    Signaled {
        signal: jobwrap_core::Signal,
        core_dumped: bool,
    },
}

impl ChildStatus {
    /// Convert to a shell-compatible status code (128 + signal for signals).
    pub fn exit_code(self) -> i32 {
        match self {
            ChildStatus::Exited { code } => code,
            ChildStatus::Signaled { signal, .. } => 128 + ffi::signal_number(signal),
        }
    }
}

/// Spawn `executable` in a fresh session with the PTY slave as its terminal.
///
/// The child becomes a session leader with its own process group and the slave
/// as its controlling terminal, then execs without any shell. Takes the slave
/// from the PTY; the master remains in `pty` for the relay.
pub fn spawn(
    pty: &mut crate::PseudoTerminal,
    window_size: Option<jobwrap_core::WindowSize>,
    executable: &Path,
    args: &[std::ffi::OsString],
    envs: &[(std::ffi::OsString, std::ffi::OsString)],
) -> io::Result<SpawnedChild> {
    // The child inherits the window size set on the master at exec time.
    if let Some(ws) = window_size {
        pty.set_window_size(ws)?;
    }
    let slave = pty
        .take_slave()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "PTY slave already taken"))?;
    let slave_fd = slave.as_raw_fd();
    let pid = ffi::fork_exec(slave_fd, executable, args, envs)?;
    // The parent no longer needs the slave descriptor; the child has its own
    // copy.
    drop(slave);
    Ok(SpawnedChild {
        pid,
        process_group: ProcessGroupId(pid.0),
        session: SessionId(pid.0),
    })
}

/// Block until the child changes state. Returns the final status after the
/// child has been reaped.
///
/// `on_change` is invoked for intermediate stop/continue transitions so the
/// caller can report state to a daemon.
pub fn wait_for_child(
    pid: ProcessId,
    on_change: &mut dyn FnMut(ChildStateChange),
) -> io::Result<ChildStatus> {
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid with WUNTRACED|WCONTINUED reports stops and resumes.
        let ret = unsafe { libc::waitpid(pid.0, &mut status, libc::WUNTRACED | libc::WCONTINUED) };
        if ret == -1 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if status_exited(status) {
            let code = exit_code(status);
            return Ok(ChildStatus::Exited { code });
        }
        if status_signaled(status) {
            let sig = signal_of(status);
            let core = core_dumped(status);
            let signal = core_to_signal(sig);
            return Ok(ChildStatus::Signaled {
                signal,
                core_dumped: core,
            });
        }
        if status_stopped(status) {
            on_change(ChildStateChange::Stopped);
            continue;
        }
        if status_continued(status) {
            on_change(ChildStateChange::Continued);
            continue;
        }
        // Defensive: any other waitpid result means the child is gone.
        return Err(io::Error::new(
            io::ErrorKind::Other,
            "waitpid returned an unexpected status",
        ));
    }
}

/// A non-terminal child state change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildStateChange {
    Stopped,
    Continued,
}

// Thin wrappers around waitpid status decoding (all safe integer ops).

fn status_exited(status: libc::c_int) -> bool {
    (status & 0x7f) == 0
}

fn status_signaled(status: libc::c_int) -> bool {
    (status & 0x7f) != 0 && !status_stopped(status)
}

fn status_stopped(status: libc::c_int) -> bool {
    ((status & 0xff) >> 8) == 0x7f
}

fn status_continued(status: libc::c_int) -> bool {
    status == 0xffff
}

fn exit_code(status: libc::c_int) -> i32 {
    (status >> 8) & 0xff
}

fn signal_of(status: libc::c_int) -> i32 {
    status & 0x7f
}

fn core_dumped(status: libc::c_int) -> bool {
    (status & 0x80) != 0
}

fn core_to_signal(sig: i32) -> jobwrap_core::Signal {
    match sig {
        libc::SIGINT => jobwrap_core::Signal::Interrupt,
        libc::SIGTERM => jobwrap_core::Signal::Terminate,
        libc::SIGHUP => jobwrap_core::Signal::Hangup,
        libc::SIGQUIT => jobwrap_core::Signal::Quit,
        libc::SIGKILL => jobwrap_core::Signal::Kill,
        _ => jobwrap_core::Signal::Terminate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_decoding() {
        assert!(status_exited(0));
        assert_eq!(exit_code(0), 0);
        assert_eq!(exit_code(0x0100), 1);
        assert!(status_signaled(0x0002));
        assert_eq!(signal_of(0x0002), libc::SIGINT);
        assert!(core_dumped(0x0082));
    }
}
