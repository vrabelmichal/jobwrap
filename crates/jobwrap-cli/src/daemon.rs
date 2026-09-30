//! Client side of the wrapper/CLI <-> daemon protocol.
//!
//! Connects over the private Unix socket, auto-starting the daemon when
//! needed, and verifying the socket peer's UID before trusting it.

use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

use jobwrap_config::RuntimePaths;
use jobwrap_protocol::{codec, ClientToDaemon, DaemonToClient, Hello, HelloRole, PROTOCOL_VERSION};
use nix::sys::socket::{self, sockopt};
use nix::unistd::Uid;

/// How much of the daemon's output to include in a startup error.
const LOG_TAIL_BYTES: usize = 2048;

/// A connection to the daemon for request/response traffic (CLI subcommands).
pub struct DaemonClient {
    write: UnixStream,
    read: UnixStream,
    pub daemon_pid: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("could not connect to the jobwrap daemon: {0}")]
    Connect(#[from] std::io::Error),
    #[error("failed to start jobwrapd: {0}")]
    Startup(String),
    #[error("the daemon socket is owned by UID {owner}, but the current UID is {current}; refusing to connect")]
    PeerUidMismatch { owner: u32, current: u32 },
    #[error("protocol error: {0}")]
    Protocol(#[from] jobwrap_protocol::ProtocolError),
    #[error("the daemon rejected our protocol version")]
    VersionRejected,
    #[error("the daemon returned an error: {0}")]
    Daemon(String),
}

impl DaemonClient {
    /// Connect to the daemon, auto-starting it if required and possible.
    pub fn connect(paths: &RuntimePaths, auto_start: bool) -> Result<Self, ClientError> {
        let stream = open_connection(paths, auto_start)?;
        let read = stream.try_clone()?;
        Self::handshake(stream, read, HelloRole::Cli)
    }

    fn handshake(
        write: UnixStream,
        read: UnixStream,
        role: HelloRole,
    ) -> Result<Self, ClientError> {
        let mut write = write;
        write.write_all(&codec::encode_frame(&ClientToDaemon::Hello(Hello {
            protocol_version: PROTOCOL_VERSION,
            role,
            pid: std::process::id() as i32,
            uid: Uid::current().as_raw(),
        }))?)?;
        let mut read = read;
        let reply = codec::read_frame(&mut read)?;
        match reply {
            DaemonToClient::HelloAck {
                protocol_version,
                daemon_pid,
            } => {
                if protocol_version != PROTOCOL_VERSION {
                    return Err(ClientError::VersionRejected);
                }
                Ok(DaemonClient {
                    write,
                    read,
                    daemon_pid,
                })
            }
            DaemonToClient::Error { message, .. } => Err(ClientError::Daemon(message)),
            other => Err(ClientError::Daemon(format!(
                "expected hello ack, got {}",
                protocol_tag(&other)
            ))),
        }
    }

    /// Send a request.
    pub fn send(&mut self, msg: &ClientToDaemon) -> Result<(), ClientError> {
        let frame = codec::encode_frame(msg)?;
        self.write.write_all(&frame)?;
        Ok(())
    }

    /// Receive one response.
    pub fn receive(&mut self) -> Result<DaemonToClient, ClientError> {
        Ok(codec::read_frame(&mut self.read)?)
    }

    /// A typed request/response exchange.
    pub fn request(&mut self, msg: ClientToDaemon) -> Result<DaemonToClient, ClientError> {
        self.send(&msg)?;
        self.receive()
    }
}

/// Open a connection for the wrapper role, returning the read and write halves
/// and the daemon PID after the Hello handshake.
pub fn connect_wrapper_stream(
    paths: &RuntimePaths,
    auto_start: bool,
) -> Result<(UnixStream, UnixStream, i32), ClientError> {
    let stream = open_connection(paths, auto_start)?;
    let read = stream.try_clone()?;
    let mut write = stream;
    write.write_all(&codec::encode_frame(&ClientToDaemon::Hello(Hello {
        protocol_version: PROTOCOL_VERSION,
        role: HelloRole::Wrapper,
        pid: std::process::id() as i32,
        uid: Uid::current().as_raw(),
    }))?)?;
    let mut read = read;
    let reply = codec::read_frame(&mut read)?;
    match reply {
        DaemonToClient::HelloAck {
            protocol_version,
            daemon_pid,
        } => {
            if protocol_version != PROTOCOL_VERSION {
                return Err(ClientError::VersionRejected);
            }
            // Wrapper connections are long lived; an idle control channel is
            // normal and must not silently expire after the request timeout.
            read.set_read_timeout(None)?;
            Ok((read, write, daemon_pid))
        }
        DaemonToClient::Error { message, .. } => Err(ClientError::Daemon(message)),
        other => Err(ClientError::Daemon(format!(
            "expected hello ack, got {}",
            protocol_tag(&other)
        ))),
    }
}

/// Connect, auto-starting the daemon when needed, and verify the peer UID.
fn open_connection(paths: &RuntimePaths, auto_start: bool) -> Result<UnixStream, ClientError> {
    let stream = match connect_socket(&paths.daemon_socket) {
        Ok(s) => s,
        Err(connect_err) => {
            if !auto_start {
                return Err(ClientError::Connect(connect_err));
            }
            start_daemon(paths, &connect_err)?;
            connect_socket(&paths.daemon_socket).map_err(ClientError::Connect)?
        }
    };
    verify_peer_uid(&stream)?;
    Ok(stream)
}

fn protocol_tag(msg: &DaemonToClient) -> &'static str {
    match msg {
        DaemonToClient::HelloAck { .. } => "hello_ack",
        DaemonToClient::Registered { .. } => "registered",
        DaemonToClient::JobList { .. } => "job_list",
        DaemonToClient::JobDetail { .. } => "job_detail",
        DaemonToClient::Output { .. } => "output",
        DaemonToClient::Ack => "ack",
        DaemonToClient::Error { .. } => "error",
        DaemonToClient::AuthStatus { .. } => "auth_status",
        DaemonToClient::TokenCreated { .. } => "token_created",
        DaemonToClient::TokenList { .. } => "token_list",
        DaemonToClient::TokenRevoked { .. } => "token_revoked",
        DaemonToClient::Launched { .. } => "launched",
        DaemonToClient::PendingLaunch { .. } => "pending_launch",
        DaemonToClient::TargetInfo { .. } => "target_info",
        DaemonToClient::ManPageSearch { .. } => "man_page_search",
        DaemonToClient::ManPage { .. } => "man_page",
        DaemonToClient::ProbePreview { .. } => "probe_preview",
        DaemonToClient::ProbeResult { .. } => "probe_result",
        DaemonToClient::ProbeDeleted { .. } => "probe_deleted",
        DaemonToClient::TerminalList { .. } => "terminal_list",
        DaemonToClient::ToWrapper(_) => "to_wrapper",
    }
}

fn connect_socket(path: &Path) -> std::io::Result<UnixStream> {
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    Ok(stream)
}

/// Verify the socket peer is owned by the current user.
fn verify_peer_uid(stream: &UnixStream) -> Result<(), ClientError> {
    let cred =
        socket::getsockopt(stream, sockopt::PeerCredentials).map_err(std::io::Error::from)?;
    let current = Uid::current().as_raw();
    if cred.uid() != current {
        return Err(ClientError::PeerUidMismatch {
            owner: cred.uid(),
            current,
        });
    }
    Ok(())
}

/// Locate the `jobwrapd` binary.
pub(crate) fn find_daemon_binary() -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("JOBWRAPD_BIN") {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().map(|d| d.join("jobwrapd"));
        if let Some(s) = sibling {
            if s.is_file() {
                return Some(s);
            }
        }
    }
    for candidate in [
        "/usr/libexec/jobwrap/jobwrapd",
        "/usr/local/libexec/jobwrapd",
    ] {
        let p = std::path::PathBuf::from(candidate);
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Start the daemon, guarding against concurrent starts with the daemon lock.
///
/// The daemon's stderr is captured in `daemon.log` under the runtime
/// directory and included in the error when startup fails.
fn start_daemon(paths: &RuntimePaths, connect_err: &std::io::Error) -> Result<(), ClientError> {
    std::fs::create_dir_all(&paths.dir).map_err(|e| {
        ClientError::Daemon(format!("could not create {}: {e}", paths.dir.display()))
    })?;

    let lock_file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&paths.daemon_lock)?;
    let mut lock = fd_lock::RwLock::new(lock_file);
    let _guard = lock
        .write()
        .map_err(|e| ClientError::Daemon(format!("lock error: {e}")))?;

    // Double-check: another process may have started the daemon while we were
    // waiting for the lock.
    if UnixStream::connect(&paths.daemon_socket).is_ok() {
        return Ok(());
    }

    let binary = find_daemon_binary().ok_or_else(|| {
        ClientError::Daemon(format!(
            "jobwrapd is not installed; {connect_err}\n\
             Set JOBWRAPD_BIN to its location."
        ))
    })?;

    let log_path = paths.dir.join("daemon.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
        .map_err(|e| ClientError::Startup(format!("could not open {}: {e}", log_path.display())))?;

    let mut command = std::process::Command::new(&binary);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::from(log_file));
    let mut child = command
        .spawn()
        .map_err(|e| ClientError::Startup(format!("failed to spawn jobwrapd: {e}")))?;

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.daemon_socket) {
            if verify_peer_uid(&stream).is_ok() {
                return Ok(());
            }
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(startup_error(
                &log_path,
                &format!("jobwrapd exited during startup ({status})"),
            ));
        }
        if std::time::Instant::now() > deadline {
            return Err(startup_error(
                &log_path,
                "jobwrapd did not create its socket in time",
            ));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Build a startup error that includes the captured daemon output.
fn startup_error(log_path: &Path, summary: &str) -> ClientError {
    let tail = log_tail(log_path);
    if tail.is_empty() {
        ClientError::Startup(format!(
            "{summary}; no output captured, run `jobwrapd --foreground` to see the error"
        ))
    } else {
        ClientError::Startup(format!("{summary}:\n{tail}"))
    }
}

/// Read at most the last `LOG_TAIL_BYTES` of a log file.
fn log_tail(path: &Path) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(LOG_TAIL_BYTES as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    let _ = file.take(LOG_TAIL_BYTES as u64).read_to_end(&mut buf);
    strip_ansi(&String::from_utf8_lossy(&buf))
        .trim()
        .to_string()
}

/// Remove ANSI escape sequences (the daemon colors its tracing output).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for end in chars.by_ref() {
                if end.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The URL to show the user for the web interface.
///
/// When the first bind target is the tailscale interface and no explicit
/// `public_base_url` was configured, the auto-derived placeholder host
/// `tailscale` is replaced with the interface's own IPv4 address, which is
/// reachable from every other node in the tailnet.
pub fn public_url(config: &jobwrap_config::EffectiveConfig) -> String {
    let base = config.server.public_base_url.trim_end_matches('/');
    let first_bind = config
        .server
        .bind
        .split(',')
        .map(str::trim)
        .find(|target| !target.is_empty())
        .unwrap_or("");
    let placeholder = format!("http://tailscale:{}", config.server.port);
    if first_bind == "tailscale" && base == placeholder {
        if let Ok(Some(address)) = jobwrap_pty::ffi::interface_ipv4("tailscale0") {
            return format!("http://{address}:{}", config.server.port);
        }
    }
    base.to_string()
}

/// Build the wrapped command environment. Currently empty; reserved for future
/// per-profile environment overrides.
pub fn build_env(_args: &crate::cli::WrapArgs) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_ansi_removes_color_escapes() {
        let text = "\u{1b}[2m2026-09-09\u{1b}[0m \u{1b}[32m INFO\u{1b}[0m: started";
        assert_eq!(strip_ansi(text), "2026-09-09  INFO: started");
    }

    #[test]
    fn strip_ansi_keeps_plain_text() {
        let text = "Error: Io(Os { code: 98, kind: AddrInUse })";
        assert_eq!(strip_ansi(text), text);
    }

    #[test]
    fn strip_ansi_handles_trailing_escape() {
        assert_eq!(strip_ansi("text \u{1b}["), "text ");
    }
}
