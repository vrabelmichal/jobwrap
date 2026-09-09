//! Command-line definitions.

use std::ffi::OsString;
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

/// The subcommand names that take priority over wrapper mode.
pub const RESERVED_SUBCOMMANDS: &[&str] = &[
    "wrap",
    "list",
    "show",
    "logs",
    "attach",
    "signal",
    "stop",
    "open",
    "daemon",
    "config",
    "auth",
    "token",
    "launch",
    "attach-launch",
    "inspect",
    "terminals",
    "version",
    "help",
];

#[derive(Debug, Parser)]
#[command(
    name = "jobwrap",
    version,
    about = "Wrap a locally launched Linux process and securely expose monitoring and narrowly authorized control.",
    after_help = "If the first word is not a jobwrap subcommand it is treated as the\n\
                  command to run, so `jobwrap python3 analysis.py` works directly.\n\
                  Use `jobwrap -- COMMAND` to make that explicit."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run a command under jobwrap (default when no subcommand matches).
    Wrap(WrapArgs),
    /// List registered jobs.
    List,
    /// Show detailed information about one job.
    Show { job: String },
    /// Print (a portion of) a job's recorded output.
    Logs {
        job: String,
        #[arg(long, default_value_t = 0)]
        offset: u64,
    },
    /// Attach interactively to a job's terminal (experimental).
    Attach { job: String },
    /// Send a named signal to a job's process group.
    Signal { job: String, signal: String },
    /// Send SIGTERM to a job's process group.
    Stop { job: String },
    /// Open the job in the browser (or the job list if no job given).
    Open { job: Option<String> },
    /// Manage the per-user daemon.
    Daemon {
        #[command(subcommand)]
        action: DaemonCommand,
    },
    /// Inspect the configuration.
    Config {
        #[command(subcommand)]
        action: ConfigCommand,
    },
    /// Manage local password authentication.
    Auth {
        #[command(subcommand)]
        action: AuthCommand,
    },
    /// Manage API tokens.
    Token {
        #[command(subcommand)]
        action: TokenCommand,
    },
    /// Launch a managed process from the API (local trusted).
    Launch(LaunchArgs),
    /// Attach the one-time launch helper (used by the daemon's terminal
    /// backends; not for interactive use).
    AttachLaunch { launch_id: String },
    /// Inspect a target's static metadata and man pages.
    Inspect { target: String },
    /// List registered terminals.
    Terminals,
}

#[derive(Debug, Subcommand)]
pub enum DaemonCommand {
    /// Report whether the daemon is running.
    Status,
    /// Start the daemon.
    Start,
    /// Stop the daemon.
    Stop,
}

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print the path of the configuration file.
    Path,
    /// Write a default configuration file.
    Init,
    /// Validate the configuration and report issues.
    Validate,
    /// Print the effective (merged) configuration.
    Effective,
    /// Explain where a resolved value came from.
    Explain { field: String },
}

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Set (or reset) the browser password.
    SetPassword,
    /// Remove the browser password.
    RemovePassword,
    /// Report authentication status.
    Status,
}

#[derive(Debug, Subcommand)]
pub enum TokenCommand {
    /// Create a new API token.
    Create(TokenCreateArgs),
    /// List tokens.
    List,
    /// Revoke a token.
    Revoke { token_id: String },
}

#[derive(Debug, Args)]
pub struct TokenCreateArgs {
    #[arg(long)]
    pub name: String,
    #[arg(long, action = clap::ArgAction::Append, value_name = "SCOPE")]
    pub scope: Vec<String>,
    #[arg(long, value_name = "JOB_ID")]
    pub job_id: Option<String>,
    #[arg(long, value_name = "RFC3339")]
    pub expires_at: Option<String>,
}

/// Arguments for launching a command via the daemon.
#[derive(Debug, Clone, Args)]
pub struct LaunchArgs {
    pub executable: String,
    #[arg(num_args = 0.., value_name = "ARG", allow_hyphen_values = true)]
    pub arguments: Vec<String>,
    #[arg(long)]
    pub working_directory: Option<String>,
    #[arg(long, short)]
    pub name: Option<String>,
    #[arg(long)]
    pub profile: Option<String>,
    #[arg(long)]
    pub idempotency_key: Option<String>,
    /// Terminal mode: managed | new_terminal | existing_terminal.
    #[arg(long)]
    pub terminal: Option<String>,
    /// Terminal id for existing_terminal mode.
    #[arg(long)]
    pub terminal_id: Option<String>,
}

/// Arguments for wrapping a command.
#[derive(Debug, Clone, Args)]
#[command(trailing_var_arg = true)]
pub struct WrapArgs {
    /// A human-readable name for the job.
    #[arg(long, short)]
    pub name: Option<String>,
    /// The access profile to use.
    #[arg(long)]
    pub profile: Option<String>,
    /// Detach from the terminal and run in the background.
    #[arg(long)]
    pub detach: bool,
    /// Do not start or use the web server.
    #[arg(long)]
    pub no_web: bool,
    /// Do not record terminal output to the log.
    #[arg(long)]
    pub no_record: bool,
    /// The command and its arguments.
    #[arg(required = true, num_args = 1.., value_name = "COMMAND", allow_hyphen_values = true, trailing_var_arg = true)]
    pub command: Vec<OsString>,
}

impl WrapArgs {
    /// Split the command into executable and arguments.
    pub fn split_command(&self) -> (OsString, Vec<OsString>) {
        let mut it = self.command.iter();
        let executable = it.next().cloned().unwrap_or_default();
        let args = it.cloned().collect();
        (executable, args)
    }
}

/// The path to a command's executable, if it looks like a path.
#[allow(dead_code)]
pub fn executable_path(executable: &OsString) -> PathBuf {
    PathBuf::from(executable)
}
