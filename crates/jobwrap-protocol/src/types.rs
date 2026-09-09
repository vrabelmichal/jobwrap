//! Shared response payload types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use jobwrap_core::{JobId, JobState, Signal, TerminalMetadata, WindowSize};

/// A compact job summary for list/detail responses.
///
/// Deliberately does not expose the full record: fields such as the full
/// command, arguments, and working directory are only returned by dedicated
/// authorized endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobSummary {
    pub id: JobId,
    pub display_name: String,
    pub profile_name: String,
    pub state: JobState,
    pub owner_uid: u32,
    pub wrapper_pid: Option<i32>,
    pub child_pid: Option<i32>,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub terminal: TerminalMetadata,
    pub last_sequence: u64,
    pub output_bytes: u64,
    pub log_truncated: bool,
    pub visibility: String,
}

/// A slice of a job's recorded output log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputSlice {
    pub job_id: JobId,
    /// Sequence number of the first frame in this slice.
    pub sequence_start: u64,
    /// Base64-encoded terminal bytes.
    pub data_base64: String,
    /// True when the slice starts before the first recorded frame.
    pub truncated: bool,
}

/// A token description returned to the CLI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub job_id: Option<JobId>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

/// A range of output requested by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutputRange {
    pub job_id: JobId,
    pub sequence_start: u64,
    pub data: Vec<u8>,
}

/// Stable machine-readable error codes used by the API and CLI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorCode {
    BadRequest,
    Unauthorized,
    PermissionDenied,
    NotFound,
    Conflict,
    RateLimited,
    Internal,
}

impl ApiErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ApiErrorCode::BadRequest => "bad_request",
            ApiErrorCode::Unauthorized => "unauthorized",
            ApiErrorCode::PermissionDenied => "permission_denied",
            ApiErrorCode::NotFound => "not_found",
            ApiErrorCode::Conflict => "conflict",
            ApiErrorCode::RateLimited => "rate_limited",
            ApiErrorCode::Internal => "internal_error",
        }
    }
}

/// A typed daemon error used across the wire and the HTTP API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonError {
    pub code: ApiErrorCode,
    pub message: String,
}

impl DaemonError {
    pub fn new(code: ApiErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Signal list kept explicit for wire stability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalWire(Signal);

#[allow(dead_code)]
impl SignalWire {
    pub fn signal(self) -> Signal {
        self.0
    }
}

/// Window size kept in core; re-exported here for convenience.
pub type TerminalSize = WindowSize;

/// A terminal slice used for live output frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveOutput {
    pub sequence: u64,
    pub data_base64: String,
}

// ---- Documentation and help-probe types ----

/// Static metadata about a target file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetInfo {
    pub original_path: String,
    pub resolved_path: String,
    pub exists: bool,
    pub file_type: Option<String>,
    pub executable: bool,
    pub shebang: Option<String>,
    pub interpreter: Option<String>,
    pub interpreter_prefix_arguments: Vec<String>,
    pub target_kind: Option<String>,
    pub mtime_nanos: Option<i64>,
    pub fingerprint: Option<String>,
    pub size_bytes: Option<u64>,
    pub package: Option<String>,
}

/// A man-page match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManPageMatch {
    pub name: String,
    pub section: String,
    pub relationship: String,
    pub content_available: bool,
}

/// A man page with formatted content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManPage {
    pub name: String,
    pub section: String,
    pub content: Option<String>,
}

/// Which kind of help probe is being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelpProbeKind {
    Interpreter,
    Executable,
    Script,
    Custom,
}

/// A request to preview a help probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpProbeRequest {
    pub target: String,
    pub probe_argument: Option<String>,
    pub kind: HelpProbeKind,
    pub idempotency_key: String,
}

/// The exact invocation a probe would run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeInvocation {
    pub executable: String,
    pub arguments: Vec<String>,
}

/// A help-probe preview.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbePreview {
    pub preview_id: String,
    pub invocation: ProbeInvocation,
    pub executes_target: bool,
    pub help_support_known: bool,
    pub warning: String,
}

/// Classification of a probe result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeClassification {
    LikelyHelpOutput,
    PossibleHelpOutput,
    ArgumentRejected,
    NoOutput,
    NormalProgramBehaviorSuspected,
    TimedOut,
    SideEffectsObserved,
    ExecutionFailed,
    Unknown,
}

impl std::fmt::Display for ProbeClassification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProbeClassification::LikelyHelpOutput => "likely_help_output",
            ProbeClassification::PossibleHelpOutput => "possible_help_output",
            ProbeClassification::ArgumentRejected => "argument_rejected",
            ProbeClassification::NoOutput => "no_output",
            ProbeClassification::NormalProgramBehaviorSuspected => "normal_behavior_suspected",
            ProbeClassification::TimedOut => "timed_out",
            ProbeClassification::SideEffectsObserved => "side_effects_observed",
            ProbeClassification::ExecutionFailed => "execution_failed",
            ProbeClassification::Unknown => "unknown",
        })
    }
}

/// A completed help probe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelpProbeResult {
    pub probe_id: String,
    pub invocation: ProbeInvocation,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub output_truncated: bool,
    pub stdout: String,
    pub stderr: String,
    pub classification: ProbeClassification,
    pub sandbox_level: String,
    pub warning: String,
}

/// The protection level of the probe sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SandboxStatus {
    pub level: String,
    pub network_isolated: bool,
    pub filesystem_isolated: bool,
    pub environment_filtered: bool,
}

/// A registered terminal that can accept a cooperative launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalInfo {
    pub terminal_id: String,
    pub owner_uid: u32,
    pub state: String,
    pub shell_type: Option<String>,
    pub working_directory: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}
