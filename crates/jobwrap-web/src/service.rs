//! The interface the daemon exposes to the web layer.

use jobwrap_core::{Event, JobId, JobRecord, Principal, Signal, WindowSize};
use jobwrap_protocol::JobSummary;
use tokio::sync::broadcast;

use crate::error::ApiError;
use crate::events::ServerEvent;

/// Public server information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub version: String,
    pub auth_required: bool,
    pub bind: String,
    pub port: u16,
}

/// The set of operations the HTTP/WS layer can perform against the daemon.
pub trait JobService: Send + Sync {
    /// Resolve a request's credentials into a principal.
    fn resolve_principal(&self, session: Option<&str>, bearer: Option<&str>) -> Principal;

    /// Attempt a password login, returning a fresh session token on success.
    fn login(&self, password: &str) -> Result<String, ApiError>;

    /// Revoke a browser session.
    fn logout(&self, session_token: &str);

    /// List jobs visible to the principal.
    fn list_jobs(&self, principal: &Principal) -> Vec<JobSummary>;

    /// The full record for a job (authorized to view status).
    fn get_job(&self, principal: &Principal, id: JobId) -> Result<JobRecord, ApiError>;

    /// Output since a sequence number (authorized to view output).
    fn get_output(
        &self,
        principal: &Principal,
        id: JobId,
        sequence_start: u64,
    ) -> Result<jobwrap_protocol::OutputSlice, ApiError>;

    /// The event history for a job.
    fn get_events(&self, principal: &Principal, id: JobId) -> Result<Vec<Event>, ApiError>;

    /// Send terminal input to a job (authorized).
    fn send_input(&self, principal: &Principal, id: JobId, data: &[u8]) -> Result<(), ApiError>;

    /// Send a signal to a job's process group (authorized).
    fn send_signal(&self, principal: &Principal, id: JobId, signal: Signal)
        -> Result<(), ApiError>;

    /// Resize a job's terminal (authorized).
    fn resize(&self, principal: &Principal, id: JobId, ws: WindowSize) -> Result<(), ApiError>;

    /// Delete a job record (authorized).
    fn delete_job(&self, principal: &Principal, id: JobId) -> Result<(), ApiError>;

    /// The broadcast sender used to stream live events.
    fn broadcast(&self) -> broadcast::Sender<ServerEvent>;

    /// Server metadata.
    fn server_info(&self) -> ServerInfo;
}
