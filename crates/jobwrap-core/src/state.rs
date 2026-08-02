//! Job state machine.
//!
//! States are an explicit enum rather than independent booleans so impossible
//! combinations cannot be represented. Transitions are validated centrally by
//! [`JobState::transition`].

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::signal::Signal;

/// The lifecycle state of a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JobState {
    /// The wrapper is connected but the child has not been observed running yet.
    Registering,
    /// The child is running in its own process group.
    Running,
    /// The child process group is stopped (SIGSTOP/SIGTSTP).
    Stopped,
    /// The child exited with a status code.
    Exited { code: i32 },
    /// The child was terminated by a signal.
    Signaled { signal: Signal },
    /// The wrapper disconnected; the job may still be alive.
    Disconnected,
    /// The wrapper has not reconnected and the record is presumed stale.
    Lost,
}

/// Error returned when an illegal state transition is attempted.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("invalid job state transition: {from} -> {to}")]
pub struct StateTransitionError {
    from: JobState,
    to: JobState,
}

impl StateTransitionError {
    pub fn from(&self) -> JobState {
        self.from
    }

    pub fn to(&self) -> JobState {
        self.to
    }
}

impl JobState {
    /// Attempt the transition `self -> to`, returning an error if it is not a
    /// legal transition of the state machine.
    pub fn transition(self, to: JobState) -> Result<JobState, StateTransitionError> {
        let allowed = match (self, to) {
            (JobState::Registering, JobState::Running) => true,
            (JobState::Running, JobState::Stopped) => true,
            (JobState::Running, JobState::Exited { .. }) => true,
            (JobState::Running, JobState::Signaled { .. }) => true,
            (JobState::Running, JobState::Disconnected) => true,
            (JobState::Running, JobState::Lost) => true,
            (JobState::Stopped, JobState::Running) => true,
            (JobState::Stopped, JobState::Signaled { .. }) => true,
            (JobState::Stopped, JobState::Disconnected) => true,
            (JobState::Disconnected, JobState::Running) => true,
            (JobState::Disconnected, JobState::Lost) => true,
            (JobState::Disconnected, JobState::Exited { .. }) => true,
            (JobState::Disconnected, JobState::Signaled { .. }) => true,
            // Terminal states.
            (JobState::Exited { .. }, JobState::Exited { .. }) => true,
            (JobState::Signaled { .. }, JobState::Exited { .. }) => true,
            // Every state may record its own message again.
            (JobState::Registering, JobState::Registering) => true,
            (JobState::Running, JobState::Running) => true,
            (JobState::Stopped, JobState::Stopped) => true,
            (JobState::Disconnected, JobState::Disconnected) => true,
            (JobState::Lost, JobState::Lost) => true,
            _ => false,
        };
        if allowed {
            Ok(to)
        } else {
            Err(StateTransitionError { from: self, to })
        }
    }

    /// Whether the job is considered finished.
    pub fn is_finished(self) -> bool {
        matches!(
            self,
            JobState::Exited { .. } | JobState::Signaled { .. } | JobState::Lost
        )
    }

    /// Whether control actions (input/signals) may currently be delivered.
    pub fn is_controllable(self) -> bool {
        matches!(
            self,
            JobState::Running | JobState::Stopped | JobState::Registering
        )
    }

    /// A short human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            JobState::Registering => "registering",
            JobState::Running => "running",
            JobState::Stopped => "stopped",
            JobState::Exited { .. } => "exited",
            JobState::Signaled { .. } => "signaled",
            JobState::Disconnected => "disconnected",
            JobState::Lost => "lost",
        }
    }

    /// A one-line description including details where relevant.
    pub fn describe(self) -> String {
        match self {
            JobState::Exited { code } => format!("exited with code {code}"),
            JobState::Signaled { signal } => format!("terminated by {signal}"),
            other => other.label().to_string(),
        }
    }
}

impl fmt::Display for JobState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_transitions() {
        let cases = [
            (JobState::Registering, JobState::Running, true),
            (JobState::Running, JobState::Stopped, true),
            (JobState::Running, JobState::Exited { code: 0 }, true),
            (
                JobState::Running,
                JobState::Signaled {
                    signal: Signal::Interrupt,
                },
                true,
            ),
            (JobState::Running, JobState::Disconnected, true),
            (JobState::Stopped, JobState::Running, true),
            (JobState::Disconnected, JobState::Lost, true),
            (JobState::Disconnected, JobState::Exited { code: 1 }, true),
            // Illegal transitions.
            (JobState::Registering, JobState::Exited { code: 0 }, false),
            (JobState::Exited { code: 0 }, JobState::Running, false),
            (JobState::Exited { code: 0 }, JobState::Stopped, false),
            (
                JobState::Signaled {
                    signal: Signal::Kill,
                },
                JobState::Stopped,
                false,
            ),
            (JobState::Lost, JobState::Running, false),
            (JobState::Stopped, JobState::Exited { code: 0 }, false),
        ];
        for (from, to, ok) in cases {
            assert_eq!(from.transition(to).is_ok(), ok, "{from} -> {to}");
        }
    }
}
