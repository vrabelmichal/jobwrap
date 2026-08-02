//! Shared error types for jobwrap-core.

use thiserror::Error;

/// Errors produced by core domain logic.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("job id error: {0}")]
    JobId(#[from] crate::JobIdError),
    #[error("job name error: {0}")]
    JobName(#[from] crate::job::JobNameError),
    #[error("state transition error: {0}")]
    State(#[from] crate::StateTransitionError),
    #[error("signal parse error: {0}")]
    Signal(#[from] crate::SignalParseError),
}
