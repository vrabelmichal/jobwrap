//! Detached local jobs use transient user services, not double-forking: a new
//! session alone would still be killed by an agent's cgroup cleanup.

use std::collections::HashMap;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use jobwrap_core::{JobId, Principal};
use jobwrap_protocol::DetachedRequest;

use crate::registry::Registry;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const CLAIM_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Default)]
pub struct DetachedStore {
    pending: Mutex<HashMap<JobId, (Instant, DetachedRequest)>>,
}

impl DetachedStore {
    fn insert(&self, id: JobId, request: DetachedRequest) -> Result<(), String> {
        let mut pending = self.pending.lock().expect("detached pending lock");
        pending.retain(|_, (at, _)| at.elapsed() < CLAIM_TIMEOUT);
        if pending.len() >= 128 {
            return Err("detached launch capacity reached".into());
        }
        pending.insert(id, (Instant::now(), request));
        Ok(())
    }

    pub fn take(&self, id: JobId) -> Option<DetachedRequest> {
        self.pending
            .lock()
            .expect("detached pending lock")
            .remove(&id)
            .filter(|(at, _)| at.elapsed() < CLAIM_TIMEOUT)
            .map(|(_, request)| request)
    }
}

pub async fn launch(
    registry: Arc<Registry>,
    owner: &Principal,
    request: DetachedRequest,
) -> Result<JobId, String> {
    if !matches!(
        jobwrap_core::authorize_global(owner, jobwrap_core::GlobalPermission::LaunchManagedProcess),
        jobwrap_core::AuthorizationDecision::Allow
    ) {
        return Err("detached process launch is not authorized".into());
    }
    validate(&registry, &request)?;
    let helper =
        crate::launch::find_jobwrap_binary().ok_or("jobwrap worker binary is not installed")?;
    let helper =
        std::fs::canonicalize(helper).map_err(|e| format!("resolving jobwrap worker: {e}"))?;
    let helper = helper
        .to_str()
        .ok_or("the installed worker path must be UTF-8")?;
    let job_id = JobId::generate().map_err(|e| e.to_string())?;
    registry.detached.insert(job_id, request)?;

    let unit = format!("jobwrap-job-{job_id}.service");
    let mut command = tokio::process::Command::new("systemd-run");
    command
        .args(["--user", "--quiet", "--collect", "--service-type=exec"])
        .arg(format!("--unit={unit}"))
        .args([
            "--property=StandardInput=null",
            "--property=StandardOutput=null",
            "--property=StandardError=journal",
        ])
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true);
    // Only control-plane paths enter the unit definition. Workload environment
    // values, including credentials, travel over the private socket, not argv.
    for name in [
        "HOME",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        if let Some(value) = std::env::var_os(name) {
            let mut assignment = std::ffi::OsString::from(format!("--setenv={name}="));
            assignment.push(value);
            command.arg(assignment);
        } else {
            // Do not inherit a different XDG setting from the user manager.
            command.arg(format!("--setenv={name}="));
        }
    }
    // systemd expands dollar signs in ExecStart; escape literal path bytes.
    command
        .arg("--")
        .arg(helper.replace('$', "$$"))
        .arg("detached-worker")
        .arg(job_id.to_string());
    let result = tokio::time::timeout(START_TIMEOUT, command.output()).await;
    match result {
        Ok(Ok(output)) if output.status.success() => {}
        Ok(Ok(output)) => {
            registry.detached.take(job_id);
            return Err(format!("detached launch requires a working systemd user manager; no foreground fallback was used: {}", String::from_utf8_lossy(&output.stderr).trim()));
        }
        Ok(Err(error)) => {
            registry.detached.take(job_id);
            return Err(format!("could not run systemd-run for detached mode: {error}; install systemd with a working user manager"));
        }
        Err(_) => {
            registry.detached.take(job_id);
            return Err(format!("service startup timed out for job {job_id}; check `jobwrap show {job_id}` and `systemctl --user status {unit}` before retrying"));
        }
    }
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if registry.get_job(owner, job_id).is_ok() {
            return Ok(job_id);
        }
        if Instant::now() >= deadline {
            registry.detached.take(job_id);
            return Err(format!("job {job_id} did not confirm registration; inspect `journalctl --user -u {unit}` before retrying"));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

fn validate(registry: &Registry, request: &DetachedRequest) -> Result<(), String> {
    jobwrap_core::JobName::new(request.name.clone()).map_err(|e| e.to_string())?;
    if !registry.config.profiles.contains_key(&request.profile) {
        return Err(format!("unknown profile `{}`", request.profile));
    }
    if request.command.is_empty()
        || request.command[0].is_empty()
        || request.command.len() > 4097
        || request.command[0].len() > 4096
        || request.command.iter().any(|v| v.contains(&0))
        || request.command.iter().map(Vec::len).sum::<usize>() > 256 * 1024
    {
        return Err("detached command exceeds size limits or contains NUL".into());
    }
    let cwd = std::ffi::OsStr::from_bytes(&request.working_directory);
    if request.working_directory.len() > 4096
        || request.working_directory.contains(&0)
        || !Path::new(cwd).is_absolute()
        || !Path::new(cwd).is_dir()
    {
        return Err("detached working directory must be an existing absolute path".into());
    }
    if request.environment.len() > 4096
        || request
            .environment
            .iter()
            .any(|(k, v)| k.is_empty() || k.contains(&0) || k.contains(&b'=') || v.contains(&0))
    {
        return Err("detached environment has invalid names or values".into());
    }
    Ok(())
}
