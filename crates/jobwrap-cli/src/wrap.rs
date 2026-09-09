//! The wrapper mode: run a command under a PTY, registering it with the daemon
//! and relaying terminal/daemon traffic.
//!
//! The child process is the priority: if the daemon is unavailable, the command
//! still runs attached to the terminal in degraded (monitoring-less) mode.

use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;

use anyhow::{bail, Context};
use jobwrap_config::EffectiveConfig;
use jobwrap_core::JobId;
use jobwrap_protocol::{
    codec, decode_input, ClientToDaemon, DaemonToClient, RegisterJob, ToWrapper,
};
use jobwrap_pty::{
    child, command_channel, run_relay, ChildStateChange, ChildStatus, DaemonCommand,
    PseudoTerminal, RelaySinks, SavedTerminal, TerminalGuard,
};

use crate::cli::WrapArgs;
use crate::daemon::{build_env, connect_wrapper_stream};

/// Run the wrapped command and return its exit status.
pub fn run(args: WrapArgs) -> anyhow::Result<i32> {
    if args.detach {
        bail!("--detach is not implemented; no command was started");
    }
    let config = load_config();
    let paths = jobwrap_config::RuntimePaths::discover().context("resolving XDG paths")?;

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
        register_with_daemon(&paths, &config, &saved, &child, identity, None)
    };

    // Print the banner.
    match &registration {
        Ok(link) => {
            eprintln!("jobwrap: registered job {}", link.job_id);
            eprintln!("jobwrap: name {job_name}");
            if config.defaults.show_job_url && !args.no_web {
                eprintln!(
                    "jobwrap: web http://{}:{}/jobs/{}",
                    config.server.bind, config.server.port, link.job_id
                );
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

    let status =
        run_relay(&pty, child, 1, Some(0), Some(&saved), sinks, cmd_rx).context("relay failure")?;

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

fn sanitize_name(name: &str) -> String {
    let cleaned: String = name.chars().map(sanitize_char).collect();
    if cleaned.is_empty() {
        "job".to_string()
    } else {
        cleaned.chars().take(128).collect()
    }
}

/// The connected daemon link used by the wrapper.
pub struct WrapperLink {
    pub job_id: JobId,
    read: std::os::unix::net::UnixStream,
    writer_tx: mpsc::SyncSender<ClientToDaemon>,
    writer_handle: std::thread::JoinHandle<()>,
    sequence: u64,
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
    let register = ClientToDaemon::RegisterJob(RegisterJob {
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
        record_output: identity.record_output,
        terminal_attached: saved.is_tty,
        terminal_size: saved.window_size,
        terminal_device: saved.device.clone(),
    });

    write
        .write_all(&codec::encode_frame(&register).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;

    let reply = codec::read_frame(&mut read).map_err(|e| e.to_string())?;
    match reply {
        DaemonToClient::Registered { .. } => {
            // Start the writer thread owning the write half.
            // Bound daemon-bound output so a slow or wedged daemon applies
            // backpressure instead of growing the wrapper without limit.
            let (writer_tx, writer_rx) = mpsc::sync_channel(256);
            let writer_handle = spawn_writer(write, writer_rx);
            Ok(WrapperLink {
                job_id,
                read,
                writer_tx,
                writer_handle,
                sequence: 0,
            })
        }
        DaemonToClient::Error { message, .. } => Err(message),
        other => Err(format!("expected registration ack, got {}", tag(&other))),
    }
}

/// A handle that reports the final child status to the daemon and waits for
/// the writer thread to drain before the wrapper process exits.
pub struct Finalizer {
    writer_tx: Option<mpsc::SyncSender<ClientToDaemon>>,
    writer_handle: Option<std::thread::JoinHandle<()>>,
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
    let WrapperLink {
        job_id,
        read,
        writer_tx,
        writer_handle,
        mut sequence,
    } = link;

    // Reader thread: daemon control messages -> relay commands.
    thread::Builder::new()
        .name("jw-daemon-reader".into())
        .spawn(move || {
            let mut read = read;
            loop {
                match codec::read_frame(&mut read) {
                    Ok(DaemonToClient::ToWrapper(cmd)) => {
                        let daemon_cmd = match cmd {
                            ToWrapper::SendInput { data_base64 } => {
                                DaemonCommand::Input(decode_input(&data_base64).unwrap_or_default())
                            }
                            ToWrapper::SendSignal { signal } => DaemonCommand::Signal(signal),
                            ToWrapper::Resize { window_size } => DaemonCommand::Resize(window_size),
                        };
                        if cmd_tx.send(daemon_cmd).is_err() {
                            break;
                        }
                    }
                    Ok(_) => continue,
                    Err(_) => break,
                }
            }
            tracing::warn!(job_id = %job_id, "daemon connection closed");
        })
        .expect("spawn daemon reader");

    let output_tx = writer_tx.clone();
    let state_tx = writer_tx.clone();
    let signal_tx = writer_tx.clone();

    let on_output = Box::new(move |data: &[u8]| {
        sequence += 1;
        let msg = ClientToDaemon::Output {
            sequence,
            data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
        };
        let _ = output_tx.send(msg);
    });

    let on_state = Box::new(move |change: ChildStateChange| {
        let msg = match change {
            ChildStateChange::Stopped => ClientToDaemon::WrapperStopped,
            ChildStateChange::Continued => ClientToDaemon::WrapperContinued,
        };
        let _ = state_tx.send(msg);
    });

    let on_signal = Box::new(move |signal: jobwrap_core::Signal| {
        let msg = match signal {
            jobwrap_core::Signal::Stop => ClientToDaemon::WrapperStopped,
            jobwrap_core::Signal::Continue => ClientToDaemon::WrapperContinued,
            _ => return,
        };
        let _ = signal_tx.send(msg);
    });

    let sinks = RelaySinks {
        on_output: Some(on_output),
        on_state: Some(on_state),
        on_signal: Some(on_signal),
    };
    let finalizer = Finalizer {
        writer_tx: Some(writer_tx),
        writer_handle: Some(writer_handle),
    };
    (sinks, Some(finalizer))
}

/// Spawn a thread writing daemon-bound frames to the socket. Exits on
/// `WrapperBye`, which the finalizer sends after the final status.
fn spawn_writer(
    write: std::os::unix::net::UnixStream,
    rx: mpsc::Receiver<ClientToDaemon>,
) -> std::thread::JoinHandle<()> {
    thread::Builder::new()
        .name("jw-daemon-writer".into())
        .spawn(move || {
            let mut write = write;
            while let Ok(msg) = rx.recv() {
                if matches!(msg, ClientToDaemon::WrapperBye) {
                    break;
                }
                if let Ok(frame) = codec::encode_frame(&msg) {
                    if write.write_all(&frame).is_err() {
                        break;
                    }
                }
            }
        })
        .expect("spawn writer")
}

/// A short human-readable status line for the final result.
fn status_display(status: ChildStatus) -> String {
    match status {
        ChildStatus::Exited { code } => format!("process exited with code {code}"),
        ChildStatus::Signaled { signal, .. } => format!("process terminated by {signal}"),
    }
}
