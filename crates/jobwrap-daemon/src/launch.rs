//! The launch service: managed, new-terminal, and existing-terminal process
//! creation, one-time launch handling, and pending-launch storage.
//!
//! Every process creation path (web, API, CLI) funnels through these
//! functions after authorization.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::Utc;
use jobwrap_core::{JobId, Principal, TerminalTarget};
use jobwrap_protocol::LaunchRequest;

use crate::registry::Registry;
use crate::terminal::{resolve_backend, to_info, HelperCommand, RegisteredTerminal};

/// A pending launch awaiting the `attach-launch` helper.
#[derive(Debug, Clone)]
pub struct PendingLaunch {
    pub launch_id: String,
    pub job_id: JobId,
    pub request: LaunchRequest,
    pub expires_at: chrono::DateTime<Utc>,
}

/// The pending-launch store plus the launch concurrency counter.
#[derive(Debug, Default)]
pub struct LaunchStore {
    pending: Mutex<HashMap<String, PendingLaunch>>,
    concurrent: Mutex<usize>,
}

impl LaunchStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn try_put(&self, pending: PendingLaunch, maximum: u64) -> bool {
        let mut entries = self.pending.lock().expect("pending lock");
        let now = Utc::now();
        entries.retain(|_, entry| entry.expires_at > now);
        if entries.len() as u64 >= maximum {
            return false;
        }
        entries.insert(pending.launch_id.clone(), pending);
        true
    }

    fn cancel(&self, launch_id: &str) {
        self.pending.lock().expect("pending lock").remove(launch_id);
    }

    /// Retrieve and invalidate a one-time launch.
    pub fn take(&self, launch_id: &str) -> Option<PendingLaunch> {
        let mut map = self.pending.lock().expect("pending lock");
        let entry = map
            .get(launch_id)
            .map(|p| Utc::now().signed_duration_since(p.expires_at) < chrono::Duration::zero());
        if entry == Some(false) {
            map.remove(launch_id);
            return None;
        }
        map.remove(launch_id)
    }

    pub fn inc_concurrent(&self, max: u64) -> bool {
        let mut n = self.concurrent.lock().expect("concurrent lock");
        if *n as u64 >= max {
            return false;
        }
        *n += 1;
        true
    }

    pub fn dec_concurrent(&self) {
        let mut n = self.concurrent.lock().expect("concurrent lock");
        *n = n.saturating_sub(1);
    }
}

/// Find the `jobwrap` helper binary (used inside terminal emulators).
pub fn find_jobwrap_binary() -> Option<String> {
    if let Ok(p) = std::env::var("JOBWRAP_BIN") {
        let p = std::path::PathBuf::from(p);
        if p.is_file() {
            return Some(p.display().to_string());
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        // Sibling of jobwrapd (dev builds).
        let sibling = exe.parent().map(|d| d.join("jobwrap"));
        if let Some(s) = sibling {
            if s.is_file() {
                return Some(s.display().to_string());
            }
        }
    }
    for candidate in ["/usr/bin/jobwrap", "/usr/local/bin/jobwrap"] {
        let p = std::path::PathBuf::from(candidate);
        if p.is_file() {
            return Some(p.display().to_string());
        }
    }
    None
}

/// Execute a launch request in the requested mode.
pub fn execute(
    registry: &Registry,
    principal: &Principal,
    req: &LaunchRequest,
) -> Result<(String, JobId), String> {
    // Managed mode: spawn directly.
    let job_id = JobId::generate().map_err(|e| e.to_string())?;
    match req.terminal_target {
        TerminalTarget::Managed => Err(
            "managed daemon launches are disabled: lifecycle supervision and PTY capture are not yet safe; use `jobwrap COMMAND`"
                .into(),
        ),
        TerminalTarget::NewTerminal { ref backend } => {
            launch_new_terminal(registry, principal, req, job_id, backend.as_deref())
                .map(|launch_id| (launch_id, job_id))
        }
        TerminalTarget::ExistingTerminal { .. } => Err(
            "existing-terminal launch is unavailable until an authenticated cooperative terminal channel is implemented"
                .into(),
        ),
    }
}

fn launch_new_terminal(
    registry: &Registry,
    principal: &Principal,
    req: &LaunchRequest,
    job_id: JobId,
    backend: Option<&str>,
) -> Result<String, String> {
    use jobwrap_core::{authorize_global, AuthorizationDecision, GlobalPermission};
    match authorize_global(principal, GlobalPermission::LaunchInNewTerminal) {
        AuthorizationDecision::Allow => {}
        AuthorizationDecision::Disabled => {
            return Err("launching in a new terminal is disabled".into())
        }
        AuthorizationDecision::Deny { .. } => {
            return Err("this token may not open new graphical terminals".into())
        }
    }

    let backend_id = match backend {
        Some(b) => {
            if !registry.config.terminal.allow_api_backend_selection {
                return Err("backend selection is not allowed by configuration".into());
            }
            b.to_string()
        }
        None => registry.config.terminal.preferred_backend.clone(),
    };
    if !matches!(backend_id.as_str(), "gnome-terminal" | "xterm") {
        return Err(format!("unknown terminal backend `{backend_id}`"));
    }
    let backend = resolve_backend(&backend_id);
    if !backend.available() {
        return Err(format!(
            "the configured terminal backend `{backend_id}` is unavailable; no process was started"
        ));
    }

    let helper_binary =
        find_jobwrap_binary().ok_or_else(|| "jobwrap helper binary not found".to_string())?;
    let launch_id = JobId::generate().map_err(|e| e.to_string())?;
    let pending = PendingLaunch {
        launch_id: launch_id.to_string(),
        job_id,
        request: req.clone(),
        expires_at: Utc::now()
            + chrono::Duration::seconds(
                i64::try_from(registry.config.launch.preview_lifetime_seconds)
                    .unwrap_or(300)
                    .clamp(1, 600),
            ),
    };
    if !registry
        .launch_store
        .try_put(pending, registry.config.launch.maximum_pending_launches)
    {
        return Err("too many pending launches".into());
    }
    if let Err(error) = backend.launch(&HelperCommand {
        launch_id: launch_id.to_string(),
        helper_binary,
    }) {
        registry.launch_store.cancel(&launch_id.to_string());
        return Err(error);
    }
    Ok(launch_id.to_string())
}

/// The `attach-launch` helper retrieves a pending launch.
pub fn attach_launch(
    registry: &Registry,
    launch_id: &str,
) -> Result<(LaunchRequest, JobId), String> {
    let pending = registry
        .launch_store
        .take(launch_id)
        .ok_or_else(|| "launch id not found or expired".to_string())?;
    Ok((pending.request, pending.job_id))
}

/// List terminals visible to a principal.
pub fn list_terminals(
    registry: &Registry,
    principal: &Principal,
) -> Vec<jobwrap_protocol::TerminalInfo> {
    let _ = principal; // all local principals see registered terminals
    registry.terminals.list().iter().map(to_info).collect()
}

/// Register a terminal (used by the cooperative shell hook).
pub fn register_terminal(registry: &Registry, terminal: RegisteredTerminal) -> Result<(), String> {
    registry.terminals.register(terminal)
}
