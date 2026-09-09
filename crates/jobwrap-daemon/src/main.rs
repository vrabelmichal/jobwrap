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

    // Start the HTTP server.
    let service = Arc::new(service::DaemonService::new(registry.clone()));
    let router = build_router(service.clone());
    let bind_addr = format!("{}:{}", config.server.bind, config.server.port);
    let mut http_task = tokio::spawn(serve_http(router, bind_addr.clone()));

    // Start the Unix socket server.
    let mut socket_task = tokio::spawn(socket::serve(
        registry.clone(),
        service.clone(),
        socket_path.clone(),
    ));

    tracing::info!(pid = %std::process::id(), socket = %socket_path.display(), http = %bind_addr, "jobwrapd started");

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

async fn serve_http(router: axum::Router, bind_addr: String) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    let addr = listener.local_addr()?;
    let std_listener = listener
        .into_std()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    axum::Server::from_tcp(std_listener)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?
        .serve(router.into_make_service())
        .await
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    tracing::info!(%addr, "http server listening");
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
