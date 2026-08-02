//! PTY integration tests: run real commands under a pseudo-terminal and verify
//! isatty, output, input, colors, and exit codes.

use std::io::{Read, Write};
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::net::UnixStream;

use jobwrap_core::WindowSize;
use jobwrap_pty::{child, command_channel, run_relay, PseudoTerminal, RelaySinks, SavedTerminal};

/// Run a command under a PTY, feeding `stdin_data`, returning `(exit_code,
/// output)`.
fn run_pty(command: &[&str], stdin_data: &[u8]) -> (i32, String) {
    let mut pty = PseudoTerminal::allocate().expect("pty");
    pty.set_window_size(WindowSize::new(24, 80))
        .expect("resize");
    let saved = SavedTerminal::capture(0);
    pty.initialize_slave(Some(&saved)).expect("slave termios");

    let executable = std::path::PathBuf::from(command[0]);
    let args: Vec<std::ffi::OsString> = command[1..].iter().map(std::ffi::OsString::from).collect();

    let child = child::spawn(
        &mut pty,
        Some(WindowSize::new(24, 80)),
        &executable,
        &args,
        &[],
    )
    .expect("spawn");

    // stdin: write end used by a feeder thread, read end passed to the relay.
    let (stdin_r, stdin_w) = UnixStream::pair().expect("pair");
    let stdin_r_fd = stdin_r.into_raw_fd();
    let stdin_data = stdin_data.to_vec();
    let mut stdin_w = stdin_w;
    std::thread::spawn(move || {
        let _ = stdin_w.write_all(&stdin_data);
        let _ = stdin_w.shutdown(std::net::Shutdown::Write);
    });

    // stdout: read end we keep, write end owned so it closes after the relay.
    let (out_r, out_w) = UnixStream::pair().expect("pair");
    let out_w_fd = out_w.into_raw_fd();

    let (_, rx) = command_channel();
    let status = run_relay(
        &pty,
        child,
        out_w_fd,
        Some(stdin_r_fd),
        None,
        RelaySinks::default(),
        rx,
    )
    .expect("relay");

    // Closing the write end unblocks the read below.
    // SAFETY: we created this descriptor and own it.
    let _out_w = unsafe { OwnedFd::from_raw_fd(out_w_fd) };
    let _stdin_r = unsafe { OwnedFd::from_raw_fd(stdin_r_fd) };
    drop(_out_w);
    drop(_stdin_r);

    let mut output = Vec::new();
    let _ = (&out_r).read_to_end(&mut output);
    (
        status.exit_code(),
        String::from_utf8_lossy(&output).into_owned(),
    )
}

#[test]
fn child_sees_a_terminal() {
    let (code, out) = run_pty(
        &["python3", "-c", "import sys; print(sys.stdout.isatty())"],
        b"",
    );
    assert_eq!(code, 0);
    assert!(
        out.trim().ends_with("True"),
        "expected isatty True, got {out:?}"
    );
}

#[test]
fn child_output_reaches_the_terminal() {
    let (code, out) = run_pty(&["printf", "hello-pty"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("hello-pty"), "got {out:?}");
}

#[test]
fn input_reaches_the_child() {
    let (code, out) = run_pty(&["bash", "-c", "read v; echo got:$v"], b"world\n");
    assert_eq!(code, 0);
    assert!(out.contains("got:world"), "got {out:?}");
}

#[test]
fn exit_code_propagates() {
    let (code, _out) = run_pty(&["bash", "-c", "exit 3"], b"");
    assert_eq!(code, 3);
}

#[test]
fn colors_pass_through() {
    let (code, out) = run_pty(&["printf", "\\x1b[31mred\\x1b[0m"], b"");
    assert_eq!(code, 0);
    assert!(out.contains("\u{1b}[31mred"), "got {out:?}");
}
