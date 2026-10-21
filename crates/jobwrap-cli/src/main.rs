//! The `jobwrap` command-line entry point.
//!
//! Jobs must be launched explicitly with `run`. Unknown
//! subcommands fail during parsing before any process or job is created.

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod daemon;
mod detached;
mod wrap;

use clap::Parser;

use crate::cli::Cli;

fn main() {
    init_tracing();
    let code = run_dispatch();
    std::process::exit(code);
}

fn run_dispatch() -> i32 {
    let cli = Cli::parse();
    match commands::dispatch(cli.command) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err:#}");
            1
        }
    }
}

fn init_tracing() {
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("jobwrap=warn")),
        )
        .with_writer(std::io::stderr)
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
}
