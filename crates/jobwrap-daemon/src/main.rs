//! The jobwrap daemon.
//!
//! `jobwrapd` is a per-user daemon. It provides the web interface, HTTP API,
//! authentication, the live job registry, log storage, and access control. The
//! wrapped process itself is always created by the `jobwrap` CLI; the daemon
//! never launches arbitrary processes.

#![forbid(unsafe_code)]

mod docs;
mod launch;
mod probe;
mod registry;
mod service;
mod socket;
mod terminal;

use std::sync::Arc;
use std::{io, os::unix::fs::FileTypeExt};

use clap::Parser;
use jobwrap_config::{ConfigPaths, EffectiveConfig, RuntimePaths};
use jobwrap_store::Store;
use jobwrap_web::build_router;

use registry::Registry;

#[derive(Debug, Parser)]
#[command(name = "jobwrapd", version, about = "The per-user jobwrap daemon.")]
struct DaemonCli {
    /// Path to the Unix socket (defaults to the XDG runtime dir).
    #[arg(long)]
    socket: Option<std::path::PathBuf>,
    /// Run in the foreground and do not detach.
    #[arg(long)]
    foreground: bool,
}

#[tokio::main]
async fn main() -> Result<(), DaemonError> {
    init_tracing();
    let cli = DaemonCli::parse();
    if !cli.foreground {
        nix::unistd::setsid().map_err(io::Error::from)?;
    }
    let config = load_config()?;
    let paths = ConfigPaths::discover()?;
    let runtime = RuntimePaths::discover()?;

    let store = Store::open(&paths.db_file)?;

    std::fs::create_dir_all(&runtime.dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&runtime.dir, std::fs::Permissions::from_mode(0o700))?;
    let socket_path = cli.socket.clone().unwrap_or(runtime.daemon_socket.clone());
    prepare_socket_path(&socket_path)?;

    write_pid_file(&runtime.daemon_pid)?;

    // The registry owns live job state and the event broadcast.
    let registry = Arc::new(Registry::new(store, runtime.clone(), config.clone()));

    // Load persisted jobs as disconnected/lost records.
    registry.load_persisted_jobs()?;

    // Start the HTTP server: one listener per bind target.
    let service = Arc::new(service::DaemonService::new(registry.clone()));
    let router = build_router(service.clone());
    let mut listeners = Vec::new();
    for address in resolve_binds(&config.server.bind) {
        match tokio::net::TcpListener::bind((address.as_str(), config.server.port)).await {
            Ok(listener) => listeners.push(listener),
            Err(error) => {
                cleanup_runtime_files(&socket_path, &runtime.daemon_pid);
                return Err(DaemonError::Io(io::Error::new(
                    io::ErrorKind::Other,
                    format!("could not bind {address}:{}: {error}", config.server.port),
                )));
            }
        }
    }
    let bound: Vec<String> = listeners
        .iter()
        .map(|listener| {
            listener
                .local_addr()
                .map(|address| address.to_string())
                .unwrap_or_else(|_| "?".to_string())
        })
        .collect();
    let mut http_task = tokio::spawn(serve_http(listeners, router));

    // Start the Unix socket server.
    let mut socket_task = tokio::spawn(socket::serve(
        registry.clone(),
        service.clone(),
        socket_path.clone(),
    ));

    tracing::info!(
        pid = %std::process::id(),
        socket = %socket_path.display(),
        http = %bound.join(", "),
        "jobwrapd started"
    );

    tokio::select! {
        _ = shutdown_signal() => {}
        result = &mut http_task => {
            socket_task.abort();
            cleanup_runtime_files(&socket_path, &runtime.daemon_pid);
            return task_ended("HTTP server", result);
        }
        result = &mut socket_task => {
            http_task.abort();
            cleanup_runtime_files(&socket_path, &runtime.daemon_pid);
            return task_ended("Unix socket server", result);
        }
    }
    tracing::info!("shutting down");
    http_task.abort();
    socket_task.abort();
    cleanup_runtime_files(&socket_path, &runtime.daemon_pid);
    Ok(())
}

/// Resolve the configured bind targets to the addresses the sockets are
/// bound to.
///
/// The value may name several comma-separated targets. `loopback` and
/// `tailscale` name a network interface; any other value is used as the
/// address itself. Duplicate addresses are bound only once.
fn resolve_binds(bind: &str) -> Vec<String> {
    let mut addresses: Vec<String> = Vec::new();
    for target in bind.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let address = match target {
            "loopback" => "127.0.0.1".to_string(),
            "tailscale" => match jobwrap_pty::ffi::interface_ipv4("tailscale0") {
                Ok(Some(address)) => address.to_string(),
                Ok(None) => {
                    tracing::warn!("tailscale0 has no IPv4 address; using loopback instead");
                    "127.0.0.1".to_string()
                }
                Err(error) => {
                    tracing::warn!(%error, "could not query tailscale0; using loopback instead");
                    "127.0.0.1".to_string()
                }
            },
            other => other.to_string(),
        };
        if !addresses.contains(&address) {
            addresses.push(address);
        }
    }
    if addresses.is_empty() {
        addresses.push("127.0.0.1".to_string());
    }
    addresses
}

fn prepare_socket_path(path: &std::path::Path) -> io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if metadata.file_type().is_symlink() || !metadata.file_type().is_socket() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("refusing to replace non-socket path {}", path.display()),
        ));
    }
    if std::os::unix::net::UnixStream::connect(path).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            format!("a daemon is already listening at {}", path.display()),
        ));
    }
    std::fs::remove_file(path)
}

fn write_pid_file(path: &std::path::Path) -> io::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() || !metadata.file_type().is_file() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("refusing to replace unsafe pid path {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(std::process::id().to_string().as_bytes())
}

fn cleanup_runtime_files(socket_path: &std::path::Path, pid_path: &std::path::Path) {
    let _ = std::fs::remove_file(socket_path);
    let current_pid = std::process::id().to_string();
    if std::fs::read_to_string(pid_path).ok().as_deref() == Some(current_pid.as_str()) {
        let _ = std::fs::remove_file(pid_path);
    }
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
}

fn task_ended(
    name: &str,
    result: Result<Result<(), io::Error>, tokio::task::JoinError>,
) -> Result<(), DaemonError> {
    match result {
        Ok(Err(error)) => Err(error.into()),
        Ok(Ok(())) => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            format!("{name} stopped unexpectedly"),
        )
        .into()),
        Err(error) => {
            Err(io::Error::new(io::ErrorKind::Other, format!("{name} task failed: {error}")).into())
        }
    }
}

/// Serve the router on every listener. The first listener to stop decides
/// the outcome; remaining servers are torn down when the process exits.
async fn serve_http(
    listeners: Vec<tokio::net::TcpListener>,
    router: axum::Router,
) -> std::io::Result<()> {
    let (report, mut reports) = tokio::sync::mpsc::channel(listeners.len());
    for listener in listeners {
        let address = listener
            .local_addr()
            .map(|address| address.to_string())
            .unwrap_or_else(|_| "?".to_string());
        let router = router.clone();
        let report = report.clone();
        tokio::spawn(async move {
            let std_listener = match listener.into_std() {
                Ok(listener) => listener,
                Err(error) => {
                    let _ = report.send(Err(error)).await;
                    return;
                }
            };
            tracing::info!(%address, "http server listening");
            let result = match axum::Server::from_tcp(std_listener) {
                Ok(server) => server
                    .serve(router.into_make_service())
                    .await
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)),
                Err(error) => Err(std::io::Error::new(std::io::ErrorKind::Other, error)),
            };
            let _ = report.send(result).await;
        });
    }
    drop(report);
    while let Some(result) = reports.recv().await {
        result?;
    }
    Ok(())
}

fn load_config() -> Result<EffectiveConfig, DaemonError> {
    let config = match jobwrap_config::load_user_config() {
        Ok(config) => config,
        Err(jobwrap_config::ConfigError::Io { source, .. })
            if source.kind() == io::ErrorKind::NotFound =>
        {
            jobwrap_config::builtin_defaults()
        }
        Err(error) => return Err(error.into()),
    };
    let issues = jobwrap_config::validate_effective(&config);
    if !issues.is_empty() {
        return Err(DaemonError::UnsafeConfiguration(
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    Ok(config)
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("jobwrapd=info")),
        )
        .with_writer(std::io::stderr)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}

#[derive(Debug, thiserror::Error)]
pub enum DaemonError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("store error: {0}")]
    Store(#[from] jobwrap_store::StoreError),
    #[error("config error: {0}")]
    Config(#[from] jobwrap_config::ConfigError),
    #[error("unsafe configuration: {0}")]
    UnsafeConfiguration(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_alias_resolves() {
        assert_eq!(resolve_binds("loopback"), vec!["127.0.0.1"]);
    }

    #[test]
    fn literal_addresses_pass_through() {
        assert_eq!(resolve_binds("127.0.0.2"), vec!["127.0.0.2"]);
        assert_eq!(resolve_binds("192.168.1.10"), vec!["192.168.1.10"]);
    }

    #[test]
    fn duplicates_are_bound_once() {
        assert_eq!(resolve_binds("loopback,127.0.0.1"), vec!["127.0.0.1"]);
        assert_eq!(resolve_binds("loopback, loopback"), vec!["127.0.0.1"]);
    }

    #[test]
    fn empty_value_falls_back_to_loopback() {
        assert_eq!(resolve_binds(""), vec!["127.0.0.1"]);
        assert_eq!(resolve_binds(" , "), vec!["127.0.0.1"]);
    }

    #[test]
    fn tailscale_resolves_the_interface_or_falls_back() {
        let address = resolve_binds("tailscale");
        if address == vec!["127.0.0.1"] {
            // No tailscale interface in this environment; the fallback fired.
        } else {
            // The tailscale CGNAT range is 100.64.0.0/10.
            assert!(address[0].starts_with("100."));
        }
    }

    #[test]
    fn multiple_targets_keep_their_order() {
        let binds = resolve_binds("loopback,tailscale");
        assert_eq!(binds[0], "127.0.0.1");
        if binds.len() == 2 {
            assert!(binds[1].starts_with("100."));
        } else {
            // No tailscale interface: the fallback collapsed onto loopback.
            assert_eq!(binds.len(), 1);
        }
    }
}
