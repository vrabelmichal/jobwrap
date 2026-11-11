//! Non-secret launch provenance, separate from current daemon identity.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuildIdentity {
    pub version: String,
    pub commit: String,
    pub dirty: bool,
}
impl BuildIdentity {
    pub fn current() -> Self {
        Self {
            version: crate::build_info::VERSION.into(),
            commit: crate::build_info::GIT_COMMIT.into(),
            dirty: crate::build_info::BUILD_DIRTY,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PythonEnvironment {
    pub kind: String,
    pub name: String,
    pub path: String,
    pub interpreter: Option<String>,
    /// This is launch context, not evidence of a later shell's environment.
    pub evidence: String,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionContext {
    pub worker_build: Option<BuildIdentity>,
    pub daemon_build: Option<BuildIdentity>,
    pub host: Option<String>,
    pub launch_mode: Option<String>,
    pub python_environment: Option<PythonEnvironment>,
}

/// Observations from the wrapper connection, not inferred workload outcomes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitoringMetadata {
    pub last_contact_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_output_at: Option<chrono::DateTime<chrono::Utc>>,
    pub disconnected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub disconnect_reason: Option<String>,
}
