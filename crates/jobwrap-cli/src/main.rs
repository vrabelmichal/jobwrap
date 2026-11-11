//! The `jobwrap` command-line entry point.
//!
//! Jobs must be launched explicitly with `run`. Unknown
//! subcommands fail during parsing before any process or job is created.

#![forbid(unsafe_code)]

// Standard printing macros panic on a closed pipe. CLI consumers commonly
// stop early (head, jq); treat that as ordinary successful consumption.
macro_rules! println {
    () => { crate::stdout_line(format_args!("")) };
    ($($arg:tt)*) => { crate::stdout_line(format_args!($($arg)*)) };
}
fn stdout_line(args: std::fmt::Arguments<'_>) {
    use std::io::Write;
    if let Err(error) = writeln!(std::io::stdout().lock(), "{args}") {
        if error.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(0);
        }
        eprintln!("jobwrap: stdout: {error}");
        std::process::exit(1);
    }
}

mod cli;
mod commands;
mod daemon;
mod detached;
mod transport;
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
    match if cli.json {
        commands::dispatch_json(cli.command)
    } else {
        commands::dispatch(cli.command)
    } {
        Ok(()) => 0,
        Err(err) => {
            if err.chain().any(|e| {
                e.downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe)
            }) {
                return 0;
            }
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
