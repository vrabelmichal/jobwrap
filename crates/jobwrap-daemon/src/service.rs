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
