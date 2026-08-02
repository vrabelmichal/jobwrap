//! The `jobwrap` command-line entry point.
//!
//! `jobwrap` is the binary users type in an ordinary terminal. If the first
//! word is a known subcommand it is treated as such; otherwise the whole
//! command line is interpreted as `jobwrap wrap -- COMMAND...`.

#![forbid(unsafe_code)]

mod cli;
mod commands;
mod daemon;
mod wrap;

use std::ffi::OsString;

use clap::Parser;

use crate::cli::{Cli, Command, WrapArgs, RESERVED_SUBCOMMANDS};

fn main() {
    init_tracing();
    let code = run_dispatch();
    std::process::exit(code);
}

/// Decide whether the first word selects a subcommand, otherwise wrap.
fn run_dispatch() -> i32 {
    let argv: Vec<OsString> = std::env::args_os().skip(1).collect();
    let first_word = argv.first().and_then(|a| a.to_str()).map(str::to_string);

    let wants_subcommand = match first_word.as_deref() {
        None => true,
        Some(s) => {
            RESERVED_SUBCOMMANDS.contains(&s) || matches!(s, "-h" | "--help" | "-V" | "--version")
        }
    };

    let result = if wants_subcommand {
        let cli = Cli::parse();
        commands::dispatch(cli.command)
    } else {
        // Treat the whole command line as a wrapped command.
        let wrap = parse_wrap_args(argv);
        commands::dispatch(Command::Wrap(wrap))
    };

    match result {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("error: {err:#}");
            eprintln!();
            if let Some(word) = first_word.as_deref() {
                eprintln!("If you intended to run a command called `{word}`, use:");
                eprintln!("  jobwrap -- {word}...");
            }
            eprintln!("If the command is still running, it remains attached to this terminal.");
            1
        }
    }
}

/// Build `WrapArgs` from raw argv (used when the first word is a command).
fn parse_wrap_args(argv: Vec<OsString>) -> WrapArgs {
    let mut cli_argv: Vec<OsString> = vec![OsString::from("jobwrap"), OsString::from("wrap")];
    cli_argv.extend(argv);
    let Command::Wrap(args) = Cli::parse_from(cli_argv).command else {
        std::process::exit(2);
    };
    args
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
