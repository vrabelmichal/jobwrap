//! Structured job events.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::state::JobState;

/// A monotonically increasing event sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct EventId(pub u64);

/// The kinds of events a job can emit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// The job was registered with the daemon.
    Registered,
    /// The job state changed.
    StateChanged { state: JobState },
    /// The wrapper connected.
    WrapperConnected,
    /// The wrapper disconnected.
    WrapperDisconnected,
    /// A control action was taken.
    Control { operation: String },
    /// The wrapper reported the final child status.
    Finished { code: i32 },
    /// Recording was disabled after reaching the maximum log size.
    LogTruncated,
    /// An audit-relevant event.
    Audit { detail: String },
}

/// A single event in a job's event stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: EventId,
    pub job_id: crate::JobId,
    pub at: DateTime<Utc>,
    pub kind: EventKind,
}

impl EventKind {
    /// A stable lowercase label for storage.
    pub fn label(&self) -> &'static str {
        match self {
            EventKind::Registered => "registered",
            EventKind::StateChanged { .. } => "state_changed",
            EventKind::WrapperConnected => "wrapper_connected",
            EventKind::WrapperDisconnected => "wrapper_disconnected",
            EventKind::Control { .. } => "control",
            EventKind::Finished { .. } => "finished",
            EventKind::LogTruncated => "log_truncated",
            EventKind::Audit { .. } => "audit",
        }
    }
}

impl Event {
    pub fn new(job_id: crate::JobId, id: EventId, kind: EventKind) -> Self {
        Self {
            id,
            job_id,
            at: Utc::now(),
            kind,
        }
    }
}
