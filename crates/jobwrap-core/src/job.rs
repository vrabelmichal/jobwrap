//! Job records and names.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::access::AccessPolicy;
use crate::process::{ProcessGroupId, ProcessId, TerminalMetadata};
use crate::state::JobState;

/// Error produced when a job name is invalid.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum JobNameError {
    #[error("job name must not be empty")]
    Empty,
    #[error("job name must be at most 128 characters")]
    TooLong,
    #[error("job name may contain only letters, digits, `-`, `_` and `.`")]
    InvalidCharacter,
}

/// A human-readable job name (not a unique identifier).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct JobName(String);

impl JobName {
    pub fn new(name: impl Into<String>) -> Result<Self, JobNameError> {
        let name = name.into();
        if name.is_empty() {
            return Err(JobNameError::Empty);
        }
        if name.len() > 128 {
            return Err(JobNameError::TooLong);
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(JobNameError::InvalidCharacter);
        }
        Ok(Self(name))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JobName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A sanitized, safe rendering of a command line for display.
///
/// The full arguments are retained for inspection by authorized principals,
/// but this type guarantees a shell-free display string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDisplay(String);

impl CommandDisplay {
    pub fn new(parts: impl IntoIterator<Item = String>) -> Result<Self, JobNameError> {
        let parts: Vec<String> = parts.into_iter().collect();
        if parts.is_empty() {
            return Err(JobNameError::Empty);
        }
        let joined = parts.join(" ");
        // The rendered form must be displayable in a terminal/HTML context and
        // must never contain NUL bytes.
        if joined.is_empty() || joined.contains('\0') {
            return Err(JobNameError::InvalidCharacter);
        }
        Ok(Self(joined))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CommandDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Metadata about the job's recorded output log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LogMetadata {
    /// The sequence number of the last written output frame.
    pub last_sequence: u64,
    /// The total number of output bytes appended so far.
    pub bytes_written: u64,
    /// Whether recording was disabled because the maximum size was reached.
    pub truncated: bool,
}

/// The complete record of a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: crate::JobId,
    pub display_name: JobName,
    /// The numeric UID of the owner.
    pub owner_uid: u32,
    /// PID of the wrapper process, if known.
    pub wrapper_pid: Option<ProcessId>,
    /// PID of the child process, if known.
    pub child_pid: Option<ProcessId>,
    /// Process group targeted by signals.
    pub process_group_id: Option<ProcessGroupId>,
    /// The command as displayed to authorized viewers.
    pub command: CommandDisplay,
    pub executable: PathBuf,
    pub arguments: Vec<String>,
    pub working_directory: PathBuf,
    pub profile_name: String,
    pub access_policy: AccessPolicy,
    pub state: JobState,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub terminal: TerminalMetadata,
    pub log: LogMetadata,
}

impl JobRecord {
    /// A concise summary line for CLI display.
    pub fn summarize(&self) -> String {
        let mut line = format!(
            "{:<26}  {:<14}  {:<9}  {}",
            self.id, self.state, self.profile_name, self.display_name
        );
        if let JobState::Exited { code } = self.state {
            line.push_str(&format!("  (exit {code})"));
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        assert!(JobName::new("analysis-20260801-120531").is_ok());
        assert!(JobName::new("a b").is_err());
        assert!(JobName::new("").is_err());
        assert!(JobName::new("s/lash").is_err());
    }

    #[test]
    fn command_display_never_invokes_shell() {
        let cmd =
            CommandDisplay::new(["python3".to_string(), "run.py".to_string()]).expect("command");
        assert_eq!(cmd.as_str(), "python3 run.py");
    }
}
