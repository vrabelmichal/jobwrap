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

/// What the daemon currently permits for daemon-side process creation.
///
/// This reports configuration state, not a grant: every launch is still
/// authorized when it is requested.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchCapabilities {
    /// `[launch] enabled` — daemon/API process creation is opt-in.
    pub enabled: bool,
    /// `require_preview` is set, but launch previews are not implemented, so
    /// every launch fails closed until the owner relaxes it.
    pub require_preview: bool,
    /// Configured profile names available for new jobs.
    pub profiles: Vec<String>,
    /// The default profile for new jobs.
    pub default_profile: String,
    /// The configured default terminal mode.
    pub default_terminal_mode: String,
    /// The configured preferred terminal backend identifier.
    pub preferred_backend: String,
    /// Whether the preferred backend emulator binary is installed.
    pub backend_available: bool,
    /// Whether web/API clients may choose among configured backends.
    pub allow_backend_selection: bool,
    /// Backend identifiers with availability, populated when selection is
    /// allowed.
    pub backends: Vec<(String, bool)>,
}

impl LaunchCapabilities {
    /// Whether the new-job form can offer working launches right now.
    pub fn launch_ready(&self) -> bool {
        self.enabled && !self.require_preview
    }
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

    /// Internal job record lookup. HTTP representations must redact fields
    /// that require stronger permissions than status access.
    fn get_job(&self, principal: &Principal, id: JobId) -> Result<JobRecord, ApiError>;

    /// Authorize a live output subscription without reading the output log.
    ///
    /// The default implementation checks through the output API. Services with
    /// a direct authorization path should override this to avoid reading and
    /// discarding log data.
    fn authorize_output(&self, principal: &Principal, id: JobId) -> Result<(), ApiError> {
        self.get_output(principal, id, 0).map(|_| ())
    }

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

    /// Launch a managed process from the API or web interface.
    fn launch(
        &self,
        principal: &Principal,
        req: jobwrap_protocol::LaunchRequest,
    ) -> Result<(String, JobId), ApiError>;

    /// Report daemon-side process-creation capabilities for UI rendering.
    fn launch_capabilities(&self) -> LaunchCapabilities;

    // ---- documentation ----

    fn identify_target(
        &self,
        principal: &Principal,
        target: &str,
    ) -> Result<jobwrap_protocol::TargetInfo, ApiError>;

    fn search_man_pages(
        &self,
        principal: &Principal,
        target: &str,
    ) -> Result<Vec<jobwrap_protocol::ManPageMatch>, ApiError>;

    fn fetch_man_page(
        &self,
        principal: &Principal,
        name: &str,
        section: Option<String>,
    ) -> Result<Option<jobwrap_protocol::ManPage>, ApiError>;

    // ---- help probes ----

    fn preview_help_probe(
        &self,
        principal: &Principal,
        req: jobwrap_protocol::HelpProbeRequest,
    ) -> Result<jobwrap_protocol::ProbePreview, ApiError>;

    fn execute_help_probe(
        &self,
        principal: &Principal,
        preview_id: &str,
    ) -> Result<jobwrap_protocol::HelpProbeResult, ApiError>;

    fn get_help_probe(
        &self,
        principal: &Principal,
        probe_id: &str,
    ) -> Result<Option<jobwrap_protocol::HelpProbeResult>, ApiError>;

    fn delete_help_probe(&self, principal: &Principal, probe_id: &str) -> Result<bool, ApiError>;

    // ---- terminals ----

    fn list_terminals(&self, principal: &Principal) -> Vec<jobwrap_protocol::TerminalInfo>;

    /// The broadcast sender used to stream live events.
    fn broadcast(&self) -> broadcast::Sender<ServerEvent>;

    /// Server metadata.
    fn server_info(&self) -> ServerInfo;
}
