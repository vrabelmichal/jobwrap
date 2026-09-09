//! Client side of the wrapper/CLI <-> daemon protocol.
//!
//! Connects over the private Unix socket, auto-starting the daemon when
//! needed, and verifying the socket peer's UID before trusting it.

use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::thread;
use std::time::Duration;

use jobwrap_config::RuntimePaths;
use jobwrap_protocol::{codec, ClientToDaemon, DaemonToClient, Hello, HelloRole, PROTOCOL_VERSION};
use nix::sys::socket::{self, sockopt};
use nix::unistd::Uid;

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

    let mut command = std::process::Command::new(&binary);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let child = command
        .spawn()
        .map_err(|e| ClientError::Daemon(format!("failed to spawn jobwrapd: {e}")))?;
    drop(child);

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(stream) = UnixStream::connect(&paths.daemon_socket) {
            if verify_peer_uid(&stream).is_ok() {
                return Ok(());
            }
        }
        if std::time::Instant::now() > deadline {
            return Err(ClientError::Connect(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "jobwrapd did not create its socket in time",
            )));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Build the wrapped command environment. Currently empty; reserved for future
/// per-profile environment overrides.
pub fn build_env(_args: &crate::cli::WrapArgs) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    Vec::new()
}
