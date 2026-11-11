//! The wrapper mode: run a command under a PTY, registering it with the daemon
//! and relaying terminal/daemon traffic.
//!
//! The child process is the priority: if the daemon is unavailable, the command
//! still runs attached to the terminal in degraded (monitoring-less) mode.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::{bail, Context};
use jobwrap_config::EffectiveConfig;
use jobwrap_core::JobId;
use jobwrap_protocol::{codec, ClientToDaemon, DaemonToClient, RegisterJob};
use jobwrap_pty::{
    child, command_channel, run_relay, ChildStatus, DaemonCommand, PseudoTerminal, RelaySinks,
    SavedTerminal, TerminalGuard,
};

use crate::cli::WrapArgs;
use crate::daemon::{build_env, connect_wrapper_stream};

/// Run the wrapped command and return its exit status.
pub fn run(args: WrapArgs) -> anyhow::Result<i32> {
    let config = load_config();
    let paths = jobwrap_config::RuntimePaths::discover().context("resolving XDG paths")?;
    if args.detach {
        return crate::detached::start(args, &config, &paths);
    }
    run_local(args, config, paths, None)
}

/// Run inside either the launch terminal or an independently started worker.
pub(crate) fn run_local(
    args: WrapArgs,
    config: EffectiveConfig,
    paths: jobwrap_config::RuntimePaths,
    job_id: Option<JobId>,
) -> anyhow::Result<i32> {
    let profile_name = args
        .profile
        .clone()
        .unwrap_or_else(|| config.defaults.profile.clone());
    let job_name = args
        .name
        .clone()
        .unwrap_or_else(|| default_job_name(&args, &config));

    // Terminal capture and guard (restored on every exit path).
    let saved = SavedTerminal::capture(0);
    let _guard = TerminalGuard::new(&saved);

    // Allocate the PTY.
    let mut pty = PseudoTerminal::allocate().context("allocating pseudo-terminal")?;
    pty.initialize_slave(Some(&saved))
        .context("initializing the pty slave")?;

    // Split the command and spawn the child before any threads exist.
    let (executable, arguments) = args.split_command();
    let executable_path = PathBuf::from(&executable);
    let envs = build_env(&args);
    let child = child::spawn(
        &mut pty,
        saved.window_size,
        &executable_path,
        &arguments,
        &envs,
    )
    .with_context(|| format!("spawning {}", executable_path.display()))?;

    // Connect to the daemon and register the job.
    let identity = JobIdentity {
        executable: executable.clone(),
        arguments: arguments.clone(),
        job_name: job_name.clone(),
        profile_name: profile_name.clone(),
        record_output: !args.no_record,
    };
    let registration = if args.no_web {
        Err("registration disabled by --no-web".to_string())
    } else {
        register_with_daemon(&paths, &config, &saved, &child, identity, job_id)
    };

    if args.detach {
        if let Err(reason) = &registration {
            // Never leave a detached workload running without a record.
            // Signal both identities in case the child has not called setsid yet.
            let pid = nix::unistd::Pid::from_raw(child.pid.0);
            let _ = nix::sys::signal::killpg(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = nix::sys::signal::kill(pid, nix::sys::signal::Signal::SIGKILL);
            let _ = child::wait_for_child(child.pid, &mut |_| {});
            bail!("detached job registration failed: {reason}");
        }
    }

    // Print the banner.
    match &registration {
        Ok(link) => {
            eprintln!("jobwrap: registered job {}", link.job_id);
            eprintln!("jobwrap: name {job_name}");
            if config.defaults.show_job_url && !args.no_web {
                let base = crate::daemon::public_url(&config);
                eprintln!("jobwrap: web {base}/jobs/{}", link.job_id);
            }
            eprintln!("jobwrap: control remains available from this terminal");
        }
        Err(reason) if args.no_web => {
            eprintln!("jobwrap: web monitoring disabled by --no-web");
            let _ = reason;
        }
        Err(reason) => {
            eprintln!("jobwrap: warning: could not register with jobwrapd: {reason}");
            eprintln!("jobwrap: the command is running attached to this terminal");
            eprintln!("jobwrap: web monitoring is unavailable");
        }
    }

    // Wire the relay. The daemon link feeds the relay's command channel.
    let (cmd_tx, cmd_rx) = command_channel();
    let (sinks, final_tx) = match registration {
        Ok(link) => build_sinks(link, cmd_tx),
        Err(_) => {
            drop(cmd_tx);
            (RelaySinks::default(), None)
        }
    };

    let status = run_relay(
        &pty,
        child,
        1,
        if args.detach { None } else { Some(0) },
        if args.detach { None } else { Some(&saved) },
        sinks,
        cmd_rx,
    )
    .context("relay failure")?;

    // Report the final status to the daemon and drain the writer (best effort,
    // but does not block forever on a dead daemon).
    if let Some(mut finalizer) = final_tx {
        finalizer.finish(status);
    }

    eprintln!("jobwrap: {}", status_display(status));
    Ok(status.exit_code())
}

/// The `attach-launch` helper mode. Started inside a new terminal by the
/// daemon's terminal backend; retrieves the pending structured launch and
/// runs it under a PTY, registering the preassigned job id.
pub fn attach_launch(launch_id: &str) -> anyhow::Result<i32> {
    let config = load_config();
    let paths = jobwrap_config::RuntimePaths::discover().context("resolving XDG paths")?;

    // Fetch the pending launch from the daemon.
    let (mut read, mut write, _daemon_pid) =
        connect_wrapper_stream(&paths, config.daemon.auto_start)
            .map_err(|e| anyhow::anyhow!("could not connect to jobwrapd: {e}"))?;
    write
        .write_all(&codec::encode_frame(&ClientToDaemon::AttachLaunch {
            launch_id: launch_id.to_string(),
        })?)
        .context("sending attach-launch request")?;
    let reply = codec::read_frame(&mut read).context("reading attach-launch response")?;
    let (request, job_id) = match reply {
        DaemonToClient::PendingLaunch { request, job_id } => (request, job_id),
        DaemonToClient::Error { message, .. } => {
            bail!("the daemon refused this launch: {message}")
        }
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    };
    drop(read);
    drop(write);

    if let Some(directory) = &request.working_directory {
        std::env::set_current_dir(directory)
            .with_context(|| format!("using launch working directory {directory}"))?;
    }

    let saved = SavedTerminal::capture(0);
    let _guard = TerminalGuard::new(&saved);
    let mut pty = PseudoTerminal::allocate().context("allocating pseudo-terminal")?;
    pty.initialize_slave(Some(&saved))
        .context("initializing the pty slave")?;

    let executable = std::ffi::OsString::from(&request.executable);
    let arguments: Vec<std::ffi::OsString> = request
        .arguments
        .iter()
        .map(std::ffi::OsString::from)
        .collect();
    let executable_path = PathBuf::from(&executable);
    let child = child::spawn(
        &mut pty,
        saved.window_size,
        &executable_path,
        &arguments,
        &[],
    )
    .with_context(|| format!("spawning {}", executable_path.display()))?;

    let profile_name = request
        .profile_name
        .clone()
        .unwrap_or_else(|| config.launch.default_profile.clone());
    let job_name = request
        .display_name
        .clone()
        .unwrap_or_else(|| default_name(&request.executable, &config));
    let identity = JobIdentity {
        executable,
        arguments,
        job_name,
        profile_name,
        record_output: true,
    };
    let registration =
        register_with_daemon(&paths, &config, &saved, &child, identity, Some(job_id));

    let (cmd_tx, cmd_rx) = command_channel();
    let (sinks, final_tx) = match registration {
        Ok(link) => build_sinks(link, cmd_tx),
        Err(reason) => {
            eprintln!("jobwrap: warning: could not register: {reason}");
            eprintln!("jobwrap: the command is still running attached to this terminal");
            drop(cmd_tx);
            (RelaySinks::default(), None)
        }
    };

    let status =
        run_relay(&pty, child, 1, Some(0), Some(&saved), sinks, cmd_rx).context("relay failure")?;
    if let Some(mut finalizer) = final_tx {
        finalizer.finish(status);
    }
    Ok(status.exit_code())
}

fn default_name(executable: &str, config: &EffectiveConfig) -> String {
    let exe = std::path::Path::new(executable)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "command".into());
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let rendered = config
        .defaults
        .job_name_template
        .replace("{executable}", &exe)
        .replace("{timestamp}", &timestamp.to_string())
        .replace("{pid}", &std::process::id().to_string());
    sanitize_name(&rendered)
}

fn tag(msg: &DaemonToClient) -> &'static str {
    match msg {
        DaemonToClient::PendingLaunch { .. } => "pending_launch",
        DaemonToClient::Launched { .. } => "launched",
        DaemonToClient::Registered { .. } => "registered",
        DaemonToClient::Error { .. } => "error",
        _ => "unexpected",
    }
}

fn load_config() -> EffectiveConfig {
    match jobwrap_config::load_user_config() {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(error = %e, "no valid configuration; using built-in defaults");
            jobwrap_config::builtin_defaults()
        }
    }
}

/// Generate a default job name from the executable and timestamp.
pub fn default_job_name(args: &WrapArgs, config: &EffectiveConfig) -> String {
    let (executable, _) = args.split_command();
    let exe = executable
        .to_string_lossy()
        .rsplit('/')
        .next()
        .unwrap_or("command")
        .chars()
        .map(sanitize_char)
        .collect::<String>();
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let rendered = config
        .defaults
        .job_name_template
        .replace("{executable}", &exe)
        .replace("{timestamp}", &timestamp.to_string())
        .replace("{pid}", &std::process::id().to_string());
    sanitize_name(&rendered)
}

fn sanitize_char(c: char) -> char {
    if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
        c
    } else {
        '_'
    }
}

pub(crate) fn sanitize_name(name: &str) -> String {
    let cleaned: String = name.chars().map(sanitize_char).collect();
    if cleaned.is_empty() {
        "job".to_string()
    } else {
        cleaned.chars().take(128).collect()
    }
}

/// The connected daemon link used by the wrapper.
pub struct WrapperLink {
    pub(crate) job_id: JobId,
    pub(crate) read: std::os::unix::net::UnixStream,
    pub(crate) register: RegisterJob,
    pub(crate) paths: jobwrap_config::RuntimePaths,
}

/// Identity information needed to register a job.
struct JobIdentity {
    executable: std::ffi::OsString,
    arguments: Vec<std::ffi::OsString>,
    job_name: String,
    profile_name: String,
    record_output: bool,
}

/// Register the job with the daemon, returning an error for degraded mode.
fn register_with_daemon(
    paths: &jobwrap_config::RuntimePaths,
    config: &EffectiveConfig,
    saved: &SavedTerminal,
    child: &jobwrap_pty::SpawnedChild,
    identity: JobIdentity,
    preassigned_job_id: Option<JobId>,
) -> Result<WrapperLink, String> {
    let (mut read, mut write, _daemon_pid) =
        connect_wrapper_stream(paths, config.daemon.auto_start).map_err(|e| e.to_string())?;

    let profile = config
        .profiles
        .get(&identity.profile_name)
        .map(|p| p.access.clone())
        .ok_or_else(|| format!("unknown profile `{}`", identity.profile_name))?;

    let args_str: Vec<String> = identity
        .arguments
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let command_str = std::iter::once(identity.executable.to_string_lossy().into_owned())
        .chain(args_str.iter().cloned())
        .collect::<Vec<_>>()
        .join(" ");

    let job_id = preassigned_job_id.unwrap_or(JobId::generate().map_err(|e| e.to_string())?);
    let register = ClientToDaemon::RegisterJob(Box::new(RegisterJob {
        id: job_id,
        display_name: sanitize_name(&identity.job_name),
        owner_uid: nix::unistd::Uid::current().as_raw(),
        wrapper_pid: std::process::id() as i32,
        child_pid: child.pid.0,
        process_group_id: child.process_group.0,
        session_id: child.session.0,
        command: command_str,
        executable: identity.executable.to_string_lossy().into_owned(),
        arguments: args_str,
        working_directory: std::env::current_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        profile_name: identity.profile_name,
        profile,
        record_output: identity.record_output && config.defaults.record_output,
        terminal_attached: saved.is_tty,
        terminal_size: saved.window_size,
        terminal_device: saved.device.clone(),
        execution: jobwrap_core::execution::ExecutionContext {
            worker_build: Some(jobwrap_core::execution::BuildIdentity::current()),
            daemon_build: None,
            host: nix::unistd::gethostname()
                .ok()
                .map(|s| s.to_string_lossy().into_owned()),
            launch_mode: Some(
                if preassigned_job_id.is_some() {
                    "managed"
                } else {
                    "foreground"
                }
                .into(),
            ),
            python_environment: crate::environment::detect(
                &identity.executable,
                &identity.arguments,
                &std::env::vars_os().collect(),
                &std::env::current_dir().unwrap_or_default(),
            ),
        },
        worker_identity: jobwrap_core::worker_identity(std::process::id() as i32).ok(),
    }));

    write
        .write_all(&codec::encode_frame(&register).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let reply = codec::read_frame(&mut read).map_err(|e| e.to_string())?;
    match reply {
        DaemonToClient::Registered { .. } => {
            let ClientToDaemon::RegisterJob(register) = register else {
                unreachable!()
            };
            Ok(WrapperLink {
                job_id,
                read,
                register: *register,
                paths: paths.clone(),
            })
        }
        DaemonToClient::Error { message, .. } => Err(message),
        other => Err(format!("expected registration ack, got {}", tag(&other))),
    }
}

/// A handle that reports the final child status to the daemon and waits for
/// the writer thread to drain before the wrapper process exits.
pub struct Finalizer {
    pub(crate) writer_tx: Option<mpsc::SyncSender<ClientToDaemon>>,
    pub(crate) writer_handle: Option<std::thread::JoinHandle<()>>,
}

impl Finalizer {
    fn finish(&mut self, status: ChildStatus) {
        let msg = match status {
            ChildStatus::Exited { code } => ClientToDaemon::WrapperExited { code },
            ChildStatus::Signaled { signal, .. } => ClientToDaemon::WrapperSignaled { signal },
        };
        // Send the final status, then a Bye that makes the writer thread flush
        // and exit, so the wrapper can terminate cleanly.
        if let Some(tx) = self.writer_tx.take() {
            let _ = tx.send(msg);
            let _ = tx.send(ClientToDaemon::WrapperBye);
        }
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }
    }
}

/// Spawn the daemon reader and writer threads and return the relay sinks.
///
/// Returns `(sinks, finalizer)` where the finalizer lets the caller report the
/// final child status after the relay returns.
fn build_sinks(
    link: WrapperLink,
    cmd_tx: mpsc::Sender<DaemonCommand>,
) -> (RelaySinks, Option<Finalizer>) {
    crate::transport::sinks(link, cmd_tx)
}

/// A short human-readable status line for the final result.
fn status_display(status: ChildStatus) -> String {
    match status {
        ChildStatus::Exited { code } => format!("process exited with code {code}"),
        ChildStatus::Signaled { signal, .. } => format!("process terminated by {signal}"),
    }
}
