//! Local detached launch and the worker started outside the caller's cgroup.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use anyhow::{bail, Context};
use jobwrap_config::{EffectiveConfig, RuntimePaths};
use jobwrap_core::JobId;
use jobwrap_protocol::{ClientToDaemon, DaemonToClient, DetachedRequest};

use crate::cli::WrapArgs;
use crate::daemon::DaemonClient;

pub fn start(
    args: WrapArgs,
    config: &EffectiveConfig,
    paths: &RuntimePaths,
) -> anyhow::Result<i32> {
    if args.no_web {
        bail!("--detach requires the daemon; --no-web cannot be combined with it");
    }
    let request = DetachedRequest {
        name: crate::wrap::sanitize_name(
            &args
                .name
                .clone()
                .unwrap_or_else(|| crate::wrap::default_job_name(&args, config)),
        ),
        profile: args
            .profile
            .unwrap_or_else(|| config.defaults.profile.clone()),
        command: args.command.iter().map(|v| v.as_bytes().to_vec()).collect(),
        working_directory: std::env::current_dir()?.as_os_str().as_bytes().to_vec(),
        environment: std::env::vars_os()
            .map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec()))
            .collect(),
        record_output: !args.no_record,
    };
    let mut client = DaemonClient::connect(paths, config.daemon.auto_start)?;
    match client.request(ClientToDaemon::DetachJob(request)).context(
        "requesting detached launch; upgrade and restart jobwrapd if it predates detached mode",
    )? {
        DaemonToClient::DetachedStarted { job_id } => {
            println!("jobwrap: registered detached job {job_id}");
            println!(
                "jobwrap: web {}/jobs/{job_id}",
                crate::daemon::public_url(config)
            );
            println!("jobwrap: user service jobwrap-job-{job_id}.service");
            println!(
                "jobwrap: inspect with `jobwrap show {job_id}`; stop with `jobwrap stop {job_id}`"
            );
            Ok(0)
        }
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        _ => bail!("unexpected detached launch response; upgrade and restart jobwrapd"),
    }
}

pub fn worker(job_id: JobId) -> anyhow::Result<i32> {
    // Resolve the control plane before restoring the caller's environment.
    // Its XDG variables must not redirect the worker to another daemon.
    let paths = RuntimePaths::discover()?;
    let mut config = match jobwrap_config::load_user_config() {
        Ok(config) => config,
        Err(jobwrap_config::ConfigError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            jobwrap_config::builtin_defaults()
        }
        Err(error) => return Err(error.into()),
    };
    config.daemon.auto_start = false;
    let mut client = DaemonClient::connect(&paths, false)?;
    let request = match client.request(ClientToDaemon::ClaimDetached { job_id })? {
        DaemonToClient::DetachedLaunch {
            job_id: assigned,
            request,
        } if assigned == job_id => request,
        DaemonToClient::Error { message, .. } => bail!("{message}"),
        _ => bail!("unexpected detached worker response"),
    };
    drop(client);

    // A systemd service has no launch terminal. Make the session independent
    // as well, unless the service manager already made us a session leader.
    if nix::unistd::getsid(None)? != nix::unistd::getpid() {
        nix::unistd::setsid()?;
    }
    for (key, _) in std::env::vars_os() {
        std::env::remove_var(key);
    }
    for (key, value) in request.environment {
        std::env::set_var(OsString::from_vec(key), OsString::from_vec(value));
    }
    std::env::set_current_dir(OsString::from_vec(request.working_directory))
        .context("using detached working directory")?;
    crate::wrap::run_local(
        WrapArgs {
            name: Some(request.name),
            profile: Some(request.profile),
            detach: true,
            no_web: false,
            no_record: !request.record_output,
            command: request
                .command
                .into_iter()
                .map(OsString::from_vec)
                .collect(),
        },
        config,
        paths,
        Some(job_id),
    )
}
