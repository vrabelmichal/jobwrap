//! Server-pushed events for WebSocket streaming.

use jobwrap_core::{JobId, JobState};

/// A live event the daemon broadcasts to WebSocket subscribers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Output {
        job_id: JobId,
        sequence: u64,
        /// Stored byte offset, or None for unrecorded/truncated live output.
        byte_offset: Option<u64>,
        data: Vec<u8>,
    },
    StateChanged {
        job_id: JobId,
        state: JobState,
    },
}
