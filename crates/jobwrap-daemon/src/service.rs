//! The `JobService` implementation backed by the registry.

use std::sync::Arc;

use jobwrap_core::{Event, JobId, JobRecord, Principal, Signal, WindowSize};
use jobwrap_protocol::JobSummary;
use tokio::sync::broadcast;

use crate::registry::Registry;
use jobwrap_web::error::ApiError;
use jobwrap_web::events::ServerEvent;
use jobwrap_web::service::{JobService, ServerInfo};

/// The web-facing facade over the registry.
pub struct DaemonService {
    registry: Arc<Registry>,
    version: String,
}

impl DaemonService {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self {
            registry,
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }
}

impl JobService for DaemonService {
    fn resolve_principal(&self, session: Option<&str>, bearer: Option<&str>) -> Principal {
        self.registry.auth().resolve_principal(session, bearer)
    }

    fn login(&self, password: &str) -> Result<String, ApiError> {
        self.registry.auth().login(password)
    }

    fn logout(&self, session_token: &str) {
        self.registry.auth().logout(session_token);
    }

    fn list_jobs(&self, principal: &Principal) -> Vec<JobSummary> {
        self.registry.list_jobs(principal)
    }

    fn get_job(&self, principal: &Principal, id: JobId) -> Result<JobRecord, ApiError> {
        self.registry.get_job(principal, id)
    }

    fn get_output(
        &self,
        principal: &Principal,
        id: JobId,
        sequence_start: u64,
    ) -> Result<jobwrap_protocol::OutputSlice, ApiError> {
        self.registry.get_output(principal, id, sequence_start)
    }

    fn get_events(&self, principal: &Principal, id: JobId) -> Result<Vec<Event>, ApiError> {
        self.registry.get_events(principal, id)
    }

    fn send_input(&self, principal: &Principal, id: JobId, data: &[u8]) -> Result<(), ApiError> {
        self.registry.send_input(principal, id, data)
    }

    fn send_signal(
        &self,
        principal: &Principal,
        id: JobId,
        signal: Signal,
    ) -> Result<(), ApiError> {
        self.registry.send_signal(principal, id, signal)
    }

    fn resize(&self, principal: &Principal, id: JobId, ws: WindowSize) -> Result<(), ApiError> {
        self.registry.resize(principal, id, ws)
    }

    fn delete_job(&self, principal: &Principal, id: JobId) -> Result<(), ApiError> {
        self.registry.delete_job(principal, id)
    }

    fn launch(
        &self,
        principal: &Principal,
        req: jobwrap_protocol::LaunchRequest,
    ) -> Result<(String, JobId), ApiError> {
        self.registry
            .launch(principal, &req)
            .map_err(|e| ApiError::new(jobwrap_protocol::ApiErrorCode::PermissionDenied, e))
    }

    fn launch_capabilities(&self) -> jobwrap_web::service::LaunchCapabilities {
        let config = &self.registry.config;
        let preferred = config.terminal.preferred_backend.clone();
        let backend_available = crate::terminal::resolve_backend(&preferred).available();
        let backends = if config.terminal.allow_api_backend_selection {
            ["gnome-terminal", "xterm"]
                .iter()
                .map(|candidate| {
                    (
                        candidate.to_string(),
                        crate::terminal::resolve_backend(candidate).available(),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        jobwrap_web::service::LaunchCapabilities {
            enabled: config.launch.enabled,
            require_preview: config.launch.require_preview,
            profiles: config.profiles.keys().cloned().collect(),
            default_profile: config.launch.default_profile.clone(),
            default_terminal_mode: config.launch.default_terminal_mode.clone(),
            preferred_backend: preferred,
            backend_available,
            allow_backend_selection: config.terminal.allow_api_backend_selection,
            backends,
        }
    }

    fn identify_target(
        &self,
        principal: &Principal,
        target: &str,
    ) -> Result<jobwrap_protocol::TargetInfo, ApiError> {
        self.registry
            .identify_target(principal, target)
            .map_err(ApiError::permission_denied)
    }

    fn search_man_pages(
        &self,
        principal: &Principal,
        target: &str,
    ) -> Result<Vec<jobwrap_protocol::ManPageMatch>, ApiError> {
        self.registry
            .search_man_pages(principal, target)
            .map_err(ApiError::permission_denied)
    }

    fn fetch_man_page(
        &self,
        principal: &Principal,
        name: &str,
        section: Option<String>,
    ) -> Result<Option<jobwrap_protocol::ManPage>, ApiError> {
        self.registry
            .fetch_man_page(principal, name, section)
            .map_err(ApiError::permission_denied)
    }

    fn preview_help_probe(
        &self,
        principal: &Principal,
        req: jobwrap_protocol::HelpProbeRequest,
    ) -> Result<jobwrap_protocol::ProbePreview, ApiError> {
        self.registry
            .preview_help_probe(principal, &req)
            .map_err(ApiError::permission_denied)
    }

    fn execute_help_probe(
        &self,
        principal: &Principal,
        preview_id: &str,
    ) -> Result<jobwrap_protocol::HelpProbeResult, ApiError> {
        self.registry
            .execute_help_probe(principal, preview_id)
            .map_err(ApiError::permission_denied)
    }

    fn get_help_probe(
        &self,
        principal: &Principal,
        probe_id: &str,
    ) -> Result<Option<jobwrap_protocol::HelpProbeResult>, ApiError> {
        self.registry
            .get_help_probe(principal, probe_id)
            .map_err(ApiError::permission_denied)
    }

    fn delete_help_probe(&self, principal: &Principal, probe_id: &str) -> Result<bool, ApiError> {
        self.registry
            .delete_help_probe(principal, probe_id)
            .map_err(ApiError::permission_denied)
    }

    fn list_terminals(&self, principal: &Principal) -> Vec<jobwrap_protocol::TerminalInfo> {
        self.registry.list_terminals(principal)
    }

    fn broadcast(&self) -> broadcast::Sender<ServerEvent> {
        self.registry.broadcast()
    }

    fn server_info(&self) -> ServerInfo {
        let config = &self.registry.config;
        ServerInfo {
            version: self.version.clone(),
            auth_required: self.registry.auth().password_set(),
            bind: config.server.bind.clone(),
            port: config.server.port,
        }
    }
}
