//! Safe wrappers around libc terminal and process primitives.
//!
//! # Safety
//!
//! This module is the only place in the workspace where `unsafe` is used. Each
//! unsafe block has a written safety invariant:
//!
//! * [`openpty`] — `openpty(3)` writes two freshly-allocated file descriptors
//!   into `amaster`/`aslave` only when it returns 0. We hand ownership of each
//!   descriptor to an `OwnedFd` (closing it on drop), so no descriptor leaks
//!   and no double-close is possible.
//! * [`ioctl`] helpers — the ioctl targets are the well-known `TIOC*` codes for
//!   termios/window-size; the passed pointers point to stack-local storage of
//!   the exact type the kernel expects for those codes.
//! * [`fork_exec`] — only called before any threads exist, so async-signal
//!   safety restrictions around allocation in the child are satisfied. The
//!   child performs only async-signal-safe operations (`setsid`, `ioctl`,
//!   `dup2`, `execvp`) before exec, and uses `_exit` on failure so the parent's
//!   stdio buffers are never flushed twice.

#![allow(unsafe_code)]

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;

use jobwrap_core::{ProcessGroupId, ProcessId, SessionId, WindowSize};

/// A saved terminal attribute structure.
pub type Termios = libc::termios;

/// A window-size structure.
pub type WinSize = libc::winsize;

/// Allocate a new pseudo-terminal, returning `(master, slave)`.
pub fn openpty() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut master: libc::c_int = -1;
    let mut slave: libc::c_int = -1;
    // SAFETY: openpty only writes amaster/aslave on success (return 0); the
    // returned descriptors are freshly allocated and owned by us from here on.
    let ret = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the descriptors are owned by us (see invariant above).
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    Ok((master, slave))
}

/// Convert a `WindowSize` to the libc representation.
pub fn window_size_to_libc(ws: &WindowSize) -> WinSize {
    libc::winsize {
        ws_row: ws.rows,
        ws_col: ws.cols,
        ws_xpixel: ws.x_pixel,
        ws_ypixel: ws.y_pixel,
    }
}

/// Convert the libc representation to a `WindowSize`.
pub fn window_size_from_libc(ws: &WinSize) -> WindowSize {
    WindowSize {
        rows: ws.ws_row,
        cols: ws.ws_col,
        x_pixel: ws.ws_xpixel,
        y_pixel: ws.ws_ypixel,
    }
}

/// Read the window size of `fd`.
pub fn get_window_size(fd: RawFd) -> io::Result<WindowSize> {
    let mut ws: WinSize = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCGWINSZ writes exactly one `winsize` into the provided
    // stack-local storage.
    let ret = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(window_size_from_libc(&ws))
}

/// Set the window size of `fd`.
pub fn set_window_size(fd: RawFd, ws: &WindowSize) -> io::Result<()> {
    let ws = window_size_to_libc(ws);
    // SAFETY: TIOCSWINSZ reads exactly one `winsize` from the provided
    // stack-local storage.
    let ret = unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Read the terminal attributes of `fd`.
pub fn tcgetattr(fd: RawFd) -> io::Result<Termios> {
    // SAFETY: tcgetattr fills exactly one `termios` struct.
    let mut termios: Termios = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::tcgetattr(fd, &mut termios) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(termios)
}

/// Set the terminal attributes of `fd`.
pub fn tcsetattr(fd: RawFd, when: libc::c_int, termios: &Termios) -> io::Result<()> {
    // SAFETY: tcsetattr reads exactly one `termios` struct.
    let ret = unsafe { libc::tcsetattr(fd, when, termios) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The foreground process group of `fd`.
pub fn tcgetpgrp(fd: RawFd) -> io::Result<ProcessGroupId> {
    // SAFETY: tcgetpgrp takes only an fd.
    let pgid = unsafe { libc::tcgetpgrp(fd) };
    if pgid == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(ProcessGroupId(pgid))
}

/// Set the foreground process group of `fd`.
pub fn tcsetpgrp(fd: RawFd, pgid: ProcessGroupId) -> io::Result<()> {
    // SAFETY: tcsetpgrp takes only an fd and a pid.
    let ret = unsafe { libc::tcsetpgrp(fd, pgid.0) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Create a new session; the caller becomes the session leader.
pub fn setsid() -> io::Result<SessionId> {
    // SAFETY: setsid takes no arguments.
    let sid = unsafe { libc::setsid() };
    if sid == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(SessionId(sid))
}

/// Set `fd` as the controlling terminal (TIOCSCTTY).
pub fn set_controlling_terminal(fd: RawFd) -> io::Result<()> {
    // SAFETY: TIOCSCTTY takes an fd; the third argument is the "force" flag.
    let ret = unsafe { libc::ioctl(fd, libc::TIOCSCTTY, 0) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Duplicate `fd` to standard input, output, and error.
pub fn dup2_std(fd: RawFd) -> io::Result<()> {
    for target in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
        // SAFETY: dup2 duplicates a valid owned descriptor into the standard
        // descriptors.
        if unsafe { libc::dup2(fd, target) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

/// Send a signal to a process group (0 == own group).
pub fn kill_process_group(pgid: ProcessGroupId, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: kill takes a valid pid and signal number.
    let ret = unsafe { libc::kill(-pgid.0, signal) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Send a signal to a single process.
pub fn kill_process(pid: ProcessId, signal: libc::c_int) -> io::Result<()> {
    // SAFETY: kill takes a valid pid and signal number.
    let ret = unsafe { libc::kill(pid.0, signal) };
    if ret != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Fork and exec the command in the child.
///
/// The parent returns `Ok(None)` is never used; on success the child never
/// returns. On fork failure `Err` is returned.
pub fn fork_exec(
    slave_fd: RawFd,
    executable: &std::path::Path,
    args: &[std::ffi::OsString],
    envs: &[(std::ffi::OsString, std::ffi::OsString)],
) -> io::Result<ProcessId> {
    // SAFETY: this function is called only before any threads are spawned, so
    // the child may safely use async-signal-safe libc calls. The child performs
    // only `setsid`, `ioctl`, `dup2`, and `execvp` before `_exit`.
    let pid = unsafe { libc::fork() };
    match pid {
        -1 => Err(io::Error::last_os_error()),
        0 => child_exec(slave_fd, executable, args, envs),
        other => Ok(ProcessId(other)),
    }
}

/// The child side of fork/exec. Never returns on success.
fn child_exec(
    slave_fd: RawFd,
    executable: &std::path::Path,
    args: &[std::ffi::OsString],
    envs: &[(std::ffi::OsString, std::ffi::OsString)],
) -> ! {
    let exit = |code: libc::c_int| -> ! {
        // SAFETY: _exit does not run destructors or flush stdio.
        unsafe { libc::_exit(code) };
    };

    // Become a session leader with the slave as our controlling terminal.
    // SAFETY: setsid is async-signal-safe.
    if unsafe { libc::setsid() } == -1 {
        exit(127);
    }
    if set_controlling_terminal(slave_fd).is_err() {
        exit(127);
    }
    // We are the session leader and process-group leader; make ourselves the
    // foreground group of the slave terminal.
    // SAFETY: tcsetpgrp is async-signal-safe and the argument is our own pgid.
    if unsafe { libc::tcsetpgrp(slave_fd, libc::getpgrp()) } == -1 {
        exit(127);
    }
    if dup2_std(slave_fd).is_err() {
        exit(127);
    }
    // SAFETY: dup2 above may have replaced the descriptor we still use; closing
    // a duplicate-owned fd is safe and does not affect 0/1/2.
    unsafe { libc::close(slave_fd) };

    // Build argv: program name followed by the arguments, null-terminated.
    let mut argv: Vec<CString> = Vec::with_capacity(args.len() + 1);
    let program = match CString::new(executable.as_os_str().as_bytes()) {
        Ok(c) => c,
        Err(_) => exit(126),
    };
    argv.push(program);
    for arg in args {
        let c = match CString::new(arg.as_bytes()) {
            Ok(c) => c,
            Err(_) => exit(126),
        };
        argv.push(c);
    }

    let mut argv_ptrs: Vec<*const libc::c_char> = argv.iter().map(|c| c.as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());

    // Build a merged environment.
    let mut env_ptrs: Vec<*const libc::c_char> = Vec::new();
    let mut env_owned: Vec<CString> = Vec::new();
    // Copy the current environment.
    for (k, v) in std::env::vars() {
        if let Ok(c) = CString::new(format!("{k}={v}")) {
            env_owned.push(c);
        }
    }
    for (k, v) in envs {
        if let Ok(c) = CString::new(format!("{}={}", k.to_string_lossy(), v.to_string_lossy())) {
            env_owned.push(c);
        }
    }
    env_ptrs.extend(env_owned.iter().map(|c| c.as_ptr()));
    env_ptrs.push(std::ptr::null());

    // SAFETY: execvp replaces the process image; argv/envp are fully
    // constructed C string arrays. On success it never returns.
    unsafe {
        libc::execvpe(argv[0].as_ptr(), argv_ptrs.as_ptr(), env_ptrs.as_ptr());
    }
    // Only reached on failure.
    let err = io::Error::last_os_error();
    let message = err.to_string();
    // Write the failure to stderr before exiting.
    let msg = format!(
        "jobwrap: failed to execute {}: {message}\n",
        executable.display()
    );
    // SAFETY: write to STDOUT_FILENO is async-signal-safe; the fd is valid.
    unsafe {
        libc::write(libc::STDOUT_FILENO, msg.as_ptr().cast(), msg.len());
    }
    exit(127);
}

/// Resolve a `jobwrap_core::Signal` to its numeric value.
pub fn signal_number(signal: jobwrap_core::Signal) -> libc::c_int {
    match signal {
        jobwrap_core::Signal::Interrupt => libc::SIGINT,
        jobwrap_core::Signal::Terminate => libc::SIGTERM,
        jobwrap_core::Signal::Hangup => libc::SIGHUP,
        jobwrap_core::Signal::Quit => libc::SIGQUIT,
        jobwrap_core::Signal::Stop => libc::SIGSTOP,
        jobwrap_core::Signal::Continue => libc::SIGCONT,
        jobwrap_core::Signal::Kill => libc::SIGKILL,
    }
}

/// Read the name of the controlling terminal device of `fd`.
pub fn ttyname(fd: RawFd) -> Option<String> {
    // SAFETY: ttyname returns a pointer to a static buffer or null.
    let ptr = unsafe { libc::ttyname(fd) };
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the returned pointer is a valid NUL-terminated string owned by
    // libc.
    let cstr = unsafe { CStr::from_ptr(ptr) };
    Some(cstr.to_string_lossy().into_owned())
}

/// Wrap a raw descriptor, taking ownership.
///
/// # Safety
///
/// The caller must own `fd` (it will be closed on drop).
pub unsafe fn own_fd(fd: RawFd) -> OwnedFd {
    // SAFETY: ownership is transferred by the caller (see invariant).
    unsafe { OwnedFd::from_raw_fd(fd) }
}

/// An all-zero termios structure.
pub fn zeroed_termios() -> Termios {
    // SAFETY: `termios` is a plain C struct of bytes; a zeroed value is valid
    // storage that we subsequently fill via `cfmakeraw`/field assignment.
    unsafe { std::mem::zeroed() }
}

/// Put a termios into raw mode in place (Linux `cfmakeraw`).
pub fn cfmakeraw(t: &mut Termios) {
    // SAFETY: cfmakeraw operates on the provided termios in place.
    unsafe { libc::cfmakeraw(t) };
}

/// A default, non-raw canonical terminal configuration.
pub fn default_termios() -> Termios {
    let mut t = zeroed_termios();
    cfmakeraw(&mut t);
    t.c_iflag |= libc::ICRNL | libc::BRKINT;
    t.c_lflag |= libc::ECHO | libc::ECHONL | libc::ICANON | libc::ISIG | libc::IEXTEN;
    t.c_lflag &= !libc::ECHOCTL;
    t.c_oflag |= libc::OPOST | libc::ONLCR;
    t.c_cc[libc::VMIN] = 1;
    t.c_cc[libc::VTIME] = 0;
    t
}

/// "Relay" mode for the wrapper's own terminal.
///
/// Echo, canonical input, flow control and output processing are disabled so
/// the PTY slave performs them instead (avoiding double echo). `ISIG` stays on
/// so Ctrl+C/Ctrl+Z still generate signals to the wrapper's foreground process
/// group, which forwards them to the child.
pub fn relay_mode_termios(current: &Termios) -> Termios {
    let mut t = *current;
    t.c_lflag &= !(libc::ICANON | libc::ECHO | libc::ECHONL | libc::IEXTEN);
    t.c_lflag |= libc::ISIG;
    t.c_iflag &= !(libc::IXON | libc::IXOFF | libc::ICRNL | libc::ISTRIP);
    t.c_oflag &= !libc::OPOST;
    t.c_cc[libc::VMIN] = 1;
    t.c_cc[libc::VTIME] = 0;
    t
}

/// Set "relay" mode on `fd` based on its current attributes.
pub fn set_relay_mode(fd: RawFd) -> io::Result<()> {
    let current = tcgetattr(fd)?;
    let relay = relay_mode_termios(&current);
    tcsetattr(fd, libc::TCSANOW, &relay)
}
