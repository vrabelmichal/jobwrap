//! The relay loop connecting the original terminal, the child PTY, and the
//! daemon.
//!
//! Threads:
//!
//! * **reader** — PTY master → original stdout (+ output sink).
//! * **stdin** — original stdin → PTY master.
//! * **commands** — daemon-issued input/signal/resize → PTY master / child.
//! * **signals** — terminal signals → child process group / PTY resize.
//!
//! The main thread reaps the child and returns its status after draining
//! remaining output. The caller is responsible for holding the terminal guard
//! that restores the original terminal state.

use std::io;
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use crate::child::{ChildStateChange, ChildStatus, SpawnedChild};
use crate::ffi;
use crate::pty::PseudoTerminal;
use crate::terminal::SavedTerminal;
use jobwrap_core::{Signal, WindowSize};

/// Commands the daemon can issue to a running wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonCommand {
    Input(Vec<u8>),
    Signal(Signal),
    Resize(WindowSize),
}

/// A boxed output sink callback.
pub type OutputSink = Box<dyn FnMut(&[u8]) + Send>;
/// A boxed child-state callback.
pub type StateSink = Box<dyn FnMut(ChildStateChange) + Send>;
/// A boxed signal callback.
pub type SignalSink = Box<dyn FnMut(Signal) + Send>;

/// Output and state sinks the wrapper plugs into the relay.
#[derive(Default)]
pub struct RelaySinks {
    /// Called with each chunk of child output (may be a daemon link).
    pub on_output: Option<OutputSink>,
    /// Called when the child stops or continues.
    pub on_state: Option<StateSink>,
    /// Called when the wrapper forwards a terminal signal to the child.
    pub on_signal: Option<SignalSink>,
}

/// Run the relay until the child exits.
pub fn run_relay(
    pty: &PseudoTerminal,
    child: SpawnedChild,
    stdout_fd: RawFd,
    stdin_fd: Option<RawFd>,
    terminal: Option<&SavedTerminal>,
    sinks: RelaySinks,
    command_rx: Receiver<DaemonCommand>,
) -> io::Result<ChildStatus> {
    let master_fd = pty.master_fd();
    let child_pgid = child.process_group;

    // Split the sinks so each thread owns only what it needs.
    let mut on_output = sinks.on_output;
    let mut on_state = sinks.on_state;
    let mut on_signal = sinks.on_signal;

    // Put the wrapper's terminal into relay mode; the caller's TerminalGuard
    // restores it on every exit path.
    if let Some(t) = terminal {
        if t.is_tty {
            ffi::set_relay_mode(t.as_raw_fd())?;
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let child_alive = Arc::new(AtomicBool::new(true));

    // --- Reader: PTY master -> stdout + sink. ---
    let reader_stop = stop.clone();
    let reader = thread::Builder::new()
        .name("jw-reader".into())
        .spawn(move || -> io::Result<()> {
            let mut buf = [0u8; 8192];
            loop {
                if reader_stop.load(Ordering::Relaxed) {
                    break;
                }
                let n = match read_raw(master_fd, &mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(_) => break, // EIO after the child closes the slave.
                };
                if let Err(e) = write_all_raw(stdout_fd, &buf[..n]) {
                    tracing::warn!(error = %e, "writing child output to terminal failed");
                    break;
                }
                if let Some(sink) = on_output.as_mut() {
                    sink(&buf[..n]);
                }
            }
            Ok(())
        })
        .map_err(io::Error::from)?;

    // --- Stdin: original stdin -> PTY master. ---
    let stdin_handle = stdin_fd.map(|fd| {
        thread::Builder::new()
            .name("jw-stdin".into())
            .spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match read_raw(fd, &mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if write_all_raw(master_fd, &buf[..n]).is_err() {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(io::Error::from)
    });

    // --- Commands from the daemon. ---
    let command_child_alive = child_alive.clone();
    let command_handle = thread::Builder::new()
        .name("jw-commands".into())
        .spawn(move || {
            while let Ok(cmd) = command_rx.recv() {
                match cmd {
                    DaemonCommand::Input(data) => {
                        let _ = write_all_raw(master_fd, &data);
                    }
                    DaemonCommand::Signal(signal) => {
                        if command_child_alive.load(Ordering::Acquire) {
                            let _ = ffi::kill_process_group(child_pgid, ffi::signal_number(signal));
                        }
                    }
                    DaemonCommand::Resize(ws) => {
                        let _ = ffi::set_window_size(master_fd, &ws);
                    }
                }
            }
        })
        .map_err(io::Error::from)?;

    // --- Terminal signals. ---
    let term_fd = terminal.filter(|t| t.is_tty).map(|t| t.as_raw_fd());
    let signal_handle = thread::Builder::new()
        .name("jw-signals".into())
        .spawn(move || {
            let mut signals = signal_hook::iterator::Signals::new([
                libc::SIGINT,
                libc::SIGQUIT,
                libc::SIGTERM,
                libc::SIGHUP,
                libc::SIGTSTP,
                libc::SIGCONT,
                libc::SIGWINCH,
                libc::SIGCHLD,
            ])
            .expect("signal registration must succeed");
            for signal in signals.forever() {
                match signal {
                    libc::SIGWINCH => {
                        if let Some(fd) = term_fd {
                            if let Ok(ws) = ffi::get_window_size(fd) {
                                let _ = ffi::set_window_size(master_fd, &ws);
                            }
                        }
                    }
                    libc::SIGCONT => {
                        let _ = ffi::kill_process_group(child_pgid, libc::SIGCONT);
                        if let Some(sink) = on_signal.as_mut() {
                            sink(Signal::Continue);
                        }
                    }
                    libc::SIGTSTP => {
                        let _ = ffi::kill_process_group(child_pgid, libc::SIGTSTP);
                        if let Some(sink) = on_signal.as_mut() {
                            sink(Signal::Stop);
                        }
                        // Suspend the wrapper too, so the shell regains control.
                        // SAFETY: raising SIGSTOP on ourselves is safe and
                        // async-signal-safe.
                        unsafe {
                            libc::raise(libc::SIGSTOP);
                        }
                    }
                    libc::SIGCHLD => {
                        // The wait loop below reaps the child.
                    }
                    other => {
                        let core_sig = core_signal(other);
                        let _ = ffi::kill_process_group(child_pgid, other);
                        if let Some(sink) = on_signal.as_mut() {
                            sink(core_sig);
                        }
                        // Stop the reader once the child dies for TERM/HUP.
                        if matches!(other, libc::SIGTERM | libc::SIGHUP) {
                            stop.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
        })
        .map_err(io::Error::from)?;

    // --- Wait for the child, reporting stop/continue transitions. ---
    let status = crate::child::wait_for_child(child.pid, &mut |change: ChildStateChange| {
        if let Some(sink) = on_state.as_mut() {
            sink(change);
        }
    })?;
    child_alive.store(false, Ordering::Release);

    // Drain any remaining child output before returning. The other threads
    // (stdin, signals, daemon commands) are deliberately not joined: the daemon
    // command thread terminates once the wrapper's final status has been
    // delivered, which happens after this function returns.
    let _ = reader.join();
    drop(command_handle);
    drop(stdin_handle);
    drop(signal_handle);

    Ok(status)
}

/// Build a channel pair for daemon commands.
pub fn command_channel() -> (Sender<DaemonCommand>, Receiver<DaemonCommand>) {
    channel()
}

/// Convenience helper to write raw input to a PTY master from outside.
pub fn write_input(pty: &PseudoTerminal, data: &[u8]) -> io::Result<()> {
    write_all_raw(pty.master_fd(), data)
}

fn core_signal(raw: i32) -> Signal {
    match raw {
        libc::SIGINT => Signal::Interrupt,
        libc::SIGTERM => Signal::Terminate,
        libc::SIGHUP => Signal::Hangup,
        libc::SIGQUIT => Signal::Quit,
        _ => Signal::Terminate,
    }
}

fn read_raw(fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
    // SAFETY: read on a valid, open descriptor into a writable buffer.
    let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(n as usize)
    }
}

fn write_all_raw(fd: RawFd, buf: &[u8]) -> io::Result<()> {
    let mut written = 0;
    while written < buf.len() {
        // SAFETY: write on a valid, open descriptor from a readable buffer.
        let n = unsafe { libc::write(fd, buf[written..].as_ptr().cast(), buf.len() - written) };
        if n < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "write returned zero bytes",
            ));
        }
        written += n as usize;
    }
    Ok(())
}
