//! Handlers for the administrative CLI subcommands.

use std::io::Write;

use anyhow::{anyhow, bail, Context};
use jobwrap_config::EffectiveConfig;
use jobwrap_protocol::{ClientToDaemon, DaemonToClient};

use crate::cli::{
    AuthCommand, Command, ConfigCommand, DaemonCommand, LaunchArgs, TokenCommand, TokenCreateArgs,
};
use crate::daemon::DaemonClient;
use crate::wrap::run;

/// Dispatch a parsed command.
pub fn dispatch(command: Command) -> anyhow::Result<()> {
    match command {
        Command::Wrap(args) => {
            let code = run(args).context("jobwrap wrapper failed")?;
            std::process::exit(code);
        }
        Command::List => list(),
        Command::Show { job } => show(&job),
        Command::Logs { job, offset } => logs(&job, offset),
        Command::Attach { job } => attach(&job),
        Command::Signal { job, signal } => send_signal(&job, &signal),
        Command::Stop { job } => send_signal(&job, "term"),
        Command::Open { job } => open(&job),
        Command::Daemon { action } => daemon(action),
        Command::Config { action } => config(action),
        Command::Auth { action } => auth(action),
        Command::Token { action } => token(action),
        Command::Launch(args) => launch(args),
        Command::AttachLaunch { launch_id } => attach_launch(&launch_id),
        Command::Inspect { target } => inspect(&target),
        Command::Terminals => terminals(),
    }
}

fn with_client<T>(f: impl FnOnce(&mut DaemonClient) -> anyhow::Result<T>) -> anyhow::Result<T> {
    let paths = jobwrap_config::RuntimePaths::discover()?;
    let config = load_effective()?;
    let mut client = DaemonClient::connect(&paths, config.daemon.auto_start)
        .context("could not connect to jobwrapd; is it running?")?;
    f(&mut client)
}

fn load_effective() -> anyhow::Result<EffectiveConfig> {
    match jobwrap_config::load_user_config() {
        Ok(cfg) => Ok(cfg),
        Err(jobwrap_config::ConfigError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            Ok(jobwrap_config::builtin_defaults())
        }
        Err(error) => Err(error.into()),
    }
}

fn ensure_ok(response: DaemonToClient) -> anyhow::Result<()> {
    match response {
        DaemonToClient::Ack => Ok(()),
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    }
}

fn tag(msg: &DaemonToClient) -> &'static str {
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

fn list() -> anyhow::Result<()> {
    with_client(|client| match client.request(ClientToDaemon::ListJobs)? {
        DaemonToClient::JobList { jobs } => {
            if jobs.is_empty() {
                println!("no jobs");
            } else {
                println!("{:<26}  {:<14}  {:<9}  NAME", "JOB ID", "STATE", "PROFILE");
                for job in jobs {
                    println!(
                        "{:<26}  {:<14}  {:<9}  {}",
                        job.id, job.state, job.profile_name, job.display_name
                    );
                }
            }
            Ok(())
        }
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    })
}

fn show(job: &str) -> anyhow::Result<()> {
    let job_id = parse_job_id(job)?;
    with_client(
        |client| match client.request(ClientToDaemon::ShowJob { job_id })? {
            DaemonToClient::JobDetail { job: Some(job) } => {
                println!("job id:             {}", job.id);
                println!("name:               {}", job.display_name);
                println!("state:              {}", job.state);
                println!("profile:            {}", job.profile_name);
                println!("started:            {}", job.started_at.to_rfc3339());
                if let Some(finished) = job.finished_at {
                    println!("finished:           {}", finished.to_rfc3339());
                }
                println!("output bytes:       {}", job.output_bytes);
                println!(
                    "terminal:           {}",
                    if job.terminal.attached {
                        "attached"
                    } else {
                        "detached"
                    }
                );
                Ok(())
            }
            DaemonToClient::JobDetail { job: None } => bail!("job {job_id} not found"),
            DaemonToClient::Error { message, .. } => bail!("{message}"),
            other => bail!("unexpected daemon response: {:?}", tag(&other)),
        },
    )
}

fn logs(job: &str, offset: u64) -> anyhow::Result<()> {
    let job_id = parse_job_id(job)?;
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    with_client(|client| {
        let response = client.request(ClientToDaemon::GetOutput {
            job_id,
            sequence_start: offset,
        })?;
        match response {
            DaemonToClient::Output { slice } => {
                if let Some(bytes) = jobwrap_protocol::decode_input(&slice.data_base64) {
                    out.write_all(&bytes)?;
                    out.flush()?;
                }
                Ok(())
            }
            DaemonToClient::Error { message, .. } => bail!("{message}"),
            other => bail!("unexpected daemon response: {:?}", tag(&other)),
        }
    })
}

fn attach(job: &str) -> anyhow::Result<()> {
    bail!(
        "attach is not implemented in this build; use `jobwrap logs {job} -f` (coming soon) or \
         open the web interface instead"
    );
}

fn send_signal(job: &str, signal: &str) -> anyhow::Result<()> {
    use std::str::FromStr;
    let job_id = parse_job_id(job)?;
    let signal =
        jobwrap_core::Signal::from_str(signal).map_err(|e| anyhow!("invalid signal: {e}"))?;
    with_client(|client| {
        let response = client.request(ClientToDaemon::SendSignal { job_id, signal })?;
        ensure_ok(response)
    })
}

fn open(job: &Option<String>) -> anyhow::Result<()> {
    let config = load_effective()?;
    let base = config.server.public_base_url.trim_end_matches('/');
    let url = match job {
        Some(job) => {
            let job_id = parse_job_id(job)?;
            format!("{base}/jobs/{job_id}")
        }
        None => format!("{base}/"),
    };
    eprintln!("jobwrap: opening {url}");
    open_browser(&url)?;
    Ok(())
}

fn open_browser(url: &str) -> anyhow::Result<()> {
    for cmd in ["xdg-open", "open"] {
        if std::process::Command::new(cmd).arg(url).spawn().is_ok() {
            return Ok(());
        }
    }
    eprintln!("jobwrap: no browser opener found; visit {url}");
    Ok(())
}

fn daemon(action: DaemonCommand) -> anyhow::Result<()> {
    let paths = jobwrap_config::RuntimePaths::discover()?;
    match action {
        DaemonCommand::Status => match DaemonClient::connect(&paths, false) {
            Ok(client) => {
                println!("running (pid {})", client.daemon_pid);
                Ok(())
            }
            Err(_) => {
                println!("not running");
                Ok(())
            }
        },
        DaemonCommand::Start => {
            let client = DaemonClient::connect(&paths, true).context("starting daemon")?;
            println!("daemon running (pid {})", client.daemon_pid);
            Ok(())
        }
        DaemonCommand::Stop => {
            // Obtain the PID from a verified live socket connection. A stale
            // pid file must never be allowed to signal an unrelated process.
            let client = DaemonClient::connect(&paths, false)
                .context("daemon is not running or its socket is unavailable")?;
            let pid = client.daemon_pid;
            nix::sys::signal::kill(
                nix::unistd::Pid::from_raw(pid),
                nix::sys::signal::Signal::SIGTERM,
            )
            .map_err(|e| anyhow!("could not signal daemon pid {pid}: {e}"))?;
            println!("shutdown requested for daemon pid {pid}");
            Ok(())
        }
    }
}

fn config(action: ConfigCommand) -> anyhow::Result<()> {
    let paths = jobwrap_config::ConfigPaths::discover()?;
    match action {
        ConfigCommand::Path => {
            println!("{}", paths.config_file.display());
            Ok(())
        }
        ConfigCommand::Init => {
            if std::fs::symlink_metadata(&paths.config_file).is_ok() {
                bail!("{} already exists", paths.config_file.display());
            }
            if let Some(parent) = paths.config_file.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&paths.config_file, DEFAULT_CONFIG)?;
            println!("wrote {}", paths.config_file.display());
            Ok(())
        }
        ConfigCommand::Validate => {
            let config = jobwrap_config::load_file(&paths.config_file).with_context(|| {
                format!("invalid configuration at {}", paths.config_file.display())
            })?;
            let issues = jobwrap_config::validate_effective(&config);
            if issues.is_empty() {
                println!("configuration is valid");
            } else {
                for issue in &issues {
                    eprintln!("error: {issue}");
                }
                bail!("configuration is unsafe and jobwrapd will refuse to start");
            }
            Ok(())
        }
        ConfigCommand::Effective => {
            let config = load_effective()?;
            println!("server.bind = {}", config.server.bind);
            println!("server.port = {}", config.server.port);
            println!("daemon.auto_start = {}", config.daemon.auto_start);
            println!("defaults.profile = {}", config.defaults.profile);
            println!(
                "defaults.job_name_template = {}",
                config.defaults.job_name_template
            );
            for (name, profile) in &config.profiles {
                println!(
                    "profiles.{name}.signal_kill = {}",
                    profile.access.signal_kill
                );
            }
            Ok(())
        }
        ConfigCommand::Explain { field } => {
            let config = load_effective()?;
            if let Some(profile) = config.profiles.get(&config.defaults.profile) {
                if let Some(source) = profile.provenance.get(field.as_str()) {
                    let access = profile_access_field(&profile.access, &field)
                        .ok_or_else(|| anyhow!("unknown access field `{field}`"))?;
                    println!("access = {access}");
                    println!("field  = profiles.{}.{}", config.defaults.profile, field);
                    println!("layer  = {}", source.layer.label());
                    println!("file   = {}", source.path);
                    return Ok(());
                }
            }
            bail!("no provenance found for field `{field}`")
        }
    }
}

fn profile_access_field(
    access: &jobwrap_core::ProfileAccess,
    field: &str,
) -> Option<jobwrap_core::AccessLevel> {
    Some(match field {
        "status" => access.status,
        "output" => access.output,
        "command" => access.command,
        "working_directory" => access.working_directory,
        "send_input" => access.send_input,
        "signal_interrupt" => access.signal_interrupt,
        "signal_terminate" => access.signal_terminate,
        "signal_stop" => access.signal_stop,
        "signal_continue" => access.signal_continue,
        "signal_kill" => access.signal_kill,
        "restart" => access.restart,
        "delete" => access.delete,
        "launch" => access.launch,
        _ => return None,
    })
}

fn auth(action: AuthCommand) -> anyhow::Result<()> {
    match action {
        AuthCommand::SetPassword => {
            let password = prompt_password()?;
            let response = with_client(|client| {
                Ok::<_, anyhow::Error>(client.request(ClientToDaemon::SetPassword { password })?)
            })?;
            ensure_ok(response)
        }
        AuthCommand::RemovePassword => {
            let response = with_client(|client| {
                Ok::<_, anyhow::Error>(client.request(ClientToDaemon::RemovePassword)?)
            })?;
            ensure_ok(response)
        }
        AuthCommand::Status => {
            let response = with_client(|client| {
                Ok::<_, anyhow::Error>(client.request(ClientToDaemon::AuthStatus)?)
            })?;
            match response {
                DaemonToClient::AuthStatus {
                    password_set,
                    token_count,
                    trust_local_owner,
                } => {
                    println!("password set: {}", password_set);
                    println!("tokens: {token_count}");
                    println!("trust local owner: {trust_local_owner}");
                    Ok(())
                }
                DaemonToClient::Error { message, .. } => bail!("{message}"),
                other => bail!("unexpected daemon response: {:?}", tag(&other)),
            }
        }
    }
}

fn prompt_password() -> anyhow::Result<String> {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        bail!("password input requires a terminal");
    }
    eprint!("new password: ");
    std::io::stderr().flush()?;
    // Read a line from the terminal with echo disabled.
    let password = read_secret()?;
    if password.is_empty() {
        bail!("empty password is not allowed");
    }
    Ok(password)
}

fn read_secret() -> anyhow::Result<String> {
    let tty_path = std::fs::read_link("/proc/self/fd/0")
        .unwrap_or_else(|_| std::path::PathBuf::from("/dev/tty"));
    let tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(tty_path)?;
    let fd = std::os::unix::io::AsRawFd::as_raw_fd(&tty);
    let saved = jobwrap_pty::SavedTerminal::capture(fd);
    let _guard = jobwrap_pty::TerminalGuard::new(&saved);
    let mut attrs = jobwrap_pty::ffi::tcgetattr(fd)?;
    attrs.c_lflag &= !libc::ECHO;
    let _ = jobwrap_pty::ffi::tcsetattr(fd, libc::TCSANOW, &attrs);
    let mut line = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(tty), &mut line)?;
    eprintln!();
    Ok(line.trim_end_matches(['\n', '\r']).to_string())
}

fn token(action: TokenCommand) -> anyhow::Result<()> {
    match action {
        TokenCommand::Create(args) => token_create(args),
        TokenCommand::List => {
            let response = with_client(|client| {
                Ok::<_, anyhow::Error>(client.request(ClientToDaemon::TokenList)?)
            })?;
            match response {
                DaemonToClient::TokenList { tokens } => {
                    for token in tokens {
                        println!(
                            "{}  {}  scopes={}  {}",
                            token.id,
                            token.name,
                            token.scopes.join(","),
                            if token.revoked { "revoked" } else { "active" }
                        );
                    }
                    Ok(())
                }
                DaemonToClient::Error { message, .. } => bail!("{message}"),
                other => bail!("unexpected daemon response: {:?}", tag(&other)),
            }
        }
        TokenCommand::Revoke { token_id } => {
            let response = with_client(|client| {
                Ok::<_, anyhow::Error>(client.request(ClientToDaemon::TokenRevoke { token_id })?)
            })?;
            ensure_ok(response)
        }
    }
}

fn token_create(args: TokenCreateArgs) -> anyhow::Result<()> {
    if args.scope.is_empty() {
        bail!("at least one --scope is required");
    }
    let expires_at = match args.expires_at {
        Some(expires) => chrono::DateTime::parse_from_rfc3339(&expires)
            .with_context(|| format!("invalid expires_at {expires:?}"))?
            .with_timezone(&chrono::Utc)
            .to_rfc3339(),
        None => String::new(),
    };
    let job_id = match args.job_id {
        Some(job) => Some(parse_job_id(&job)?),
        None => None,
    };
    let response = with_client(|client| {
        Ok::<_, anyhow::Error>(client.request(ClientToDaemon::TokenCreate(
            jobwrap_protocol::TokenCreateRequest {
                name: args.name.clone(),
                scopes: args.scope.clone(),
                job_id,
                expires_at: if expires_at.is_empty() {
                    None
                } else {
                    Some(expires_at)
                },
            },
        ))?)
    })?;
    match response {
        DaemonToClient::TokenCreated { token_id, token } => {
            println!("token id: {token_id}");
            println!("token:");
            println!("{token}");
            eprintln!("jobwrap: the token is shown only once; store it securely.");
            Ok(())
        }
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    }
}

fn parse_job_id(raw: &str) -> anyhow::Result<jobwrap_core::JobId> {
    raw.parse()
        .map_err(|e: jobwrap_core::JobIdError| anyhow!("invalid job id: {e}"))
}

fn attach_launch(launch_id: &str) -> anyhow::Result<()> {
    let code = crate::wrap::attach_launch(launch_id).context("attach-launch failed")?;
    std::process::exit(code);
}

fn inspect(target: &str) -> anyhow::Result<()> {
    let response = with_client(|client| {
        Ok::<_, anyhow::Error>(client.request(ClientToDaemon::IdentifyTarget {
            target: target.to_string(),
        })?)
    })?;
    let info = match response {
        DaemonToClient::TargetInfo { info } => info,
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    };
    println!("target:        {}", info.original_path);
    println!("resolved:      {}", info.resolved_path);
    println!("exists:        {}", info.exists);
    if let Some(ft) = info.file_type {
        println!("file type:     {ft}");
    }
    println!("executable:    {}", info.executable);
    if let Some(shebang) = info.shebang {
        println!("shebang:       {shebang}");
    }
    if let Some(interp) = info.interpreter {
        println!("interpreter:   {interp}");
    }
    if let Some(kind) = info.target_kind {
        println!("target kind:   {kind}");
    }
    if let Some(pkg) = info.package {
        println!("package:       {pkg}");
    }

    // Man-page matches.
    let response = with_client(|client| {
        Ok::<_, anyhow::Error>(client.request(ClientToDaemon::SearchManPages {
            target: target.to_string(),
            section: None,
        })?)
    })?;
    let matches = match response {
        DaemonToClient::ManPageSearch { matches } => matches,
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    };
    if matches.is_empty() {
        println!("man pages:     (none found)");
    } else {
        println!("man pages:");
        for m in matches {
            println!("  {} ({}) [{}]", m.name, m.section, m.relationship);
        }
    }
    Ok(())
}

fn terminals() -> anyhow::Result<()> {
    let response = with_client(|client| {
        Ok::<_, anyhow::Error>(client.request(ClientToDaemon::ListTerminals)?)
    })?;
    match response {
        DaemonToClient::TerminalList { terminals } => {
            if terminals.is_empty() {
                println!("no terminals registered");
            } else {
                println!(
                    "{:<26}  {:<12}  {:<8}  CWD",
                    "TERMINAL ID", "STATE", "SHELL"
                );
                for t in terminals {
                    println!(
                        "{:<26}  {:<12}  {:<8}  {}",
                        t.terminal_id,
                        t.state,
                        t.shell_type.unwrap_or_else(|| "-".into()),
                        t.working_directory.unwrap_or_else(|| "-".into())
                    );
                }
            }
            Ok(())
        }
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    }
}

fn launch(args: LaunchArgs) -> anyhow::Result<()> {
    let config = load_effective()?;
    let key = args.idempotency_key.clone().unwrap_or_else(|| {
        jobwrap_core::JobId::generate()
            .map(|id| id.to_string())
            .unwrap_or_default()
    });
    let terminal_mode = args
        .terminal
        .as_deref()
        .unwrap_or(&config.launch.default_terminal_mode);
    let terminal_target = match terminal_mode {
        "managed" => jobwrap_core::TerminalTarget::Managed,
        "new-terminal" | "new_terminal" => {
            jobwrap_core::TerminalTarget::NewTerminal { backend: None }
        }
        "existing-terminal" | "existing_terminal" => {
            let terminal_id = args
                .terminal_id
                .clone()
                .ok_or_else(|| anyhow!("--terminal-id is required for existing_terminal mode"))?;
            jobwrap_core::TerminalTarget::ExistingTerminal { terminal_id }
        }
        other => bail!("unknown terminal mode `{other}` (expected managed, new-terminal, or existing-terminal)"),
    };
    let response = with_client(|client| {
        Ok::<_, anyhow::Error>(client.request(ClientToDaemon::LaunchJob(
            jobwrap_protocol::LaunchRequest {
                executable: args.executable.clone(),
                arguments: args.arguments.clone(),
                working_directory: args.working_directory.clone(),
                display_name: args.name.clone(),
                profile_name: args.profile.clone(),
                idempotency_key: key,
                terminal_target,
            },
        ))?)
    })?;
    match response {
        DaemonToClient::Launched { launch_id, job_id } => {
            println!("launch_id: {launch_id}");
            println!("job_id: {job_id}");
            println!("state: pending terminal startup");
            eprintln!("jobwrap: use `jobwrap show {job_id}` for details");
            Ok(())
        }
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        other => bail!("unexpected daemon response: {:?}", tag(&other)),
    }
}

/// Default configuration written by `jobwrap config init`.
pub const DEFAULT_CONFIG: &str = r#"# jobwrap configuration (config_version 1)
config_version = 1

[server]
bind = "127.0.0.1"
port = 8765
public_base_url = "http://127.0.0.1:8765"
open_browser_on_start = false
allow_remote_bind = false

[daemon]
auto_start = true
idle_shutdown_minutes = 0

[defaults]
profile = "standard"
allocate_pty = true
record_output = true
retain_completed_days = 30
maximum_log_bytes = 536870912
show_job_url = true
job_name_template = "{executable}-{timestamp}"

[authentication]
trust_local_owner = true
browser_session_minutes = 720
password_attempt_limit = 5
password_attempt_window_seconds = 60

# Daemon/API process creation is experimental and disabled by default. Normal
# `jobwrap COMMAND` wrapping is unaffected.
[launch]
enabled = false
default_profile = "standard"
default_terminal_mode = "new-terminal"
require_preview = true
require_idempotency_key = true
maximum_concurrent_jobs = 20
maximum_pending_launches = 20
preview_lifetime_seconds = 300

[help]
assume_help_available = false
prefer_man_pages = true
allow_interpreter_probes = false
allow_script_probes = false
default_probe_argument = "--help"
probe_timeout_seconds = 3
probe_output_limit_bytes = 1048576
cache_results = true

# The standard profile is public: anyone on the local network can watch
# output. Public output may itself contain secrets; choose the private
# profile with --profile private for sensitive work.
[profiles.standard]
status = "public"
output = "public"
command = "authenticated"
working_directory = "authenticated"
send_input = "controller"
signal_interrupt = "controller"
signal_terminate = "controller"
signal_stop = "controller"
signal_continue = "controller"
signal_kill = "owner"
restart = "owner"
delete = "owner"
launch = "owner"

[profiles.private]
status = "authenticated"
output = "authenticated"
command = "authenticated"
working_directory = "authenticated"
send_input = "owner"
signal_interrupt = "owner"
signal_terminate = "owner"
signal_stop = "owner"
signal_continue = "owner"
signal_kill = "owner"
restart = "owner"
delete = "owner"
launch = "owner"
"#;
