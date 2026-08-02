//! The jobwrap daemon.
//!
//! `jobwrapd` is a per-user daemon. It provides the web interface, HTTP API,
//! authentication, the live job registry, log storage, and access control. The
//! wrapped process itself is always created by the `jobwrap` CLI; the daemon
//! never launches arbitrary processes.

#![forbid(unsafe_code)]

mod registry;
mod service;
mod socket;

use std::sync::Arc;

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
    let config = load_config()?;
    let paths = ConfigPaths::discover()?;
    let runtime = RuntimePaths::discover()?;

    let store = Store::open(&paths.db_file)?;

    // If a daemon is already running, exit quietly (the starter waits for the
    // socket, which already exists).
    if std::os::unix::net::UnixStream::connect(&runtime.daemon_socket).is_ok() {
        return Ok(());
    }

    std::fs::create_dir_all(&runtime.dir)?;
    let socket_path = cli.socket.clone().unwrap_or(runtime.daemon_socket.clone());
    let _ = std::fs::remove_file(&socket_path);

    // Write the daemon pid.
    std::fs::write(&runtime.daemon_pid, std::process::id().to_string())?;

    // The registry owns live job state and the event broadcast.
    let registry = Arc::new(Registry::new(store, runtime.clone(), config.clone()));

    // Load persisted jobs as disconnected/lost records.
    registry.load_persisted_jobs()?;

    // Start the HTTP server.
    let service = Arc::new(service::DaemonService::new(registry.clone()));
    let router = build_router(service.clone());
    let bind_addr = format!("{}:{}", config.server.bind, config.server.port);
    let http_task = tokio::spawn(serve_http(router, bind_addr.clone()));

    // Start the Unix socket server.
    let socket_task = tokio::spawn(socket::serve(
        registry.clone(),
        service.clone(),
        socket_path.clone(),
    ));

    tracing::info!(pid = %std::process::id(), socket = %socket_path.display(), http = %bind_addr, "jobwrapd started");

    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutting down");
    http_task.abort();
    socket_task.abort();
    let _ = std::fs::remove_file(&socket_path);
    Ok(())
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
    match jobwrap_config::load_user_config() {
        Ok(cfg) => Ok(cfg),
        Err(e) => {
            tracing::warn!(error = %e, "no valid configuration; using built-in defaults");
            Ok(jobwrap_config::builtin_defaults())
        }
    }
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
}
