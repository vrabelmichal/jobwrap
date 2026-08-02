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
