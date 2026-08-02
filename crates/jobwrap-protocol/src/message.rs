//! Protocol messages.
//!
//! `ClientToDaemon` covers both CLI requests and wrapper control traffic. The
//! daemon answers with [`DaemonToClient`]. Wrapper-specific output streaming
//! uses [`WrapperToDaemon`] and daemon-to-wrapper control uses
//! [`DaemonToClient::ToWrapper`].

use serde::{Deserialize, Serialize};

use jobwrap_core::{JobId, JobState, Signal, WindowSize};

use super::types::JobSummary;

/// The version of the wire protocol.
pub const PROTOCOL_VERSION: u32 = 1;

/// The role of the peer that opened a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelloRole {
    /// A `jobwrap` CLI or wrapper process.
    Cli,
    /// The local wrapper attached to a job's terminal.
    Wrapper,
}

/// The first frame a client sends on connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub role: HelloRole,
    pub pid: i32,
    pub uid: u32,
}

/// A message a client (CLI or wrapper) sends to the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientToDaemon {
    Hello(Hello),

    // Job queries.
    ListJobs,
    ShowJob { job_id: JobId },
    GetOutput { job_id: JobId, sequence_start: u64 },

    // Control.
    SendInput { job_id: JobId, data_base64: String },
    SendSignal { job_id: JobId, signal: Signal },

    // Wrapper output streaming.
    Output { sequence: u64, data_base64: String },

    // Wrapper lifecycle/state reports (flat variants so the nested enums do
    // not collide on the `type` tag).
    WrapperReady,
    WrapperStopped,
    WrapperContinued,
    WrapperExited { code: i32 },
    WrapperSignaled { signal: Signal },
    WrapperTerminalLost,
    WrapperBye,

    // Local authentication management (CLI only, trusted socket).
    AuthStatus,
    SetPassword { password: String },
    RemovePassword,
    TokenCreate(TokenCreateRequest),
    TokenList,
    TokenRevoke { token_id: String },

    // Wrapper registration and lifecycle.
    RegisterJob(RegisterJob),
}

/// Request to create an API token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCreateRequest {
    pub name: String,
    pub scopes: Vec<String>,
    pub job_id: Option<JobId>,
    pub expires_at: Option<String>,
}

/// Job registration payload sent by the wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterJob {
    pub id: JobId,
    pub display_name: String,
    pub owner_uid: u32,
    pub wrapper_pid: i32,
    pub child_pid: i32,
    pub process_group_id: i32,
    pub session_id: i32,
    pub command: String,
    pub executable: String,
    pub arguments: Vec<String>,
    pub working_directory: String,
    pub profile_name: String,
    pub profile: jobwrap_core::ProfileAccess,
    pub terminal_attached: bool,
    pub terminal_size: Option<WindowSize>,
    pub terminal_device: Option<String>,
}

/// Wrapper-to-daemon messages sent after registration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WrapperToDaemon {
    /// The wrapper and its child are both running.
    Ready,
    /// The child process group stopped (SIGTSTP/SIGSTOP).
    Stopped,
    /// The child resumed.
    Continued,
    /// The child exited with the given code.
    Exited { code: i32 },
    /// The child was terminated by a signal.
    Signaled { signal: Signal },
    /// The wrapper's terminal connection was lost but the child continues.
    TerminalLost,
    /// The wrapper is shutting down and will not reconnect.
    Bye,
}

/// The job registration result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterResult {
    pub job_id: JobId,
    pub sequence_start: u64,
}

/// A message the daemon sends to a client or wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DaemonToClient {
    /// Acknowledgment of the client's Hello.
    HelloAck {
        protocol_version: u32,
        daemon_pid: i32,
    },

    /// Registration accepted; the job is live.
    Registered(RegisterResult),

    JobList {
        jobs: Vec<JobSummary>,
    },
    JobDetail {
        job: Option<JobSummary>,
    },
    Output {
        slice: super::OutputSlice,
    },
    Ack,
    Error {
        code: String,
        message: String,
    },

    AuthStatus {
        password_set: bool,
        token_count: usize,
        trust_local_owner: bool,
    },
    TokenCreated {
        token_id: String,
        token: String,
    },
    TokenList {
        tokens: Vec<super::types::TokenInfo>,
    },
    TokenRevoked {
        token_id: String,
    },

    /// Control messages directed at a wrapper connection.
    ToWrapper(ToWrapper),
}

/// Control messages the daemon sends to a wrapper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ToWrapper {
    SendInput { data_base64: String },
    SendSignal { signal: Signal },
    Resize { window_size: WindowSize },
}

/// Build a `SendInput` message from raw bytes.
pub fn input_message(job_id: JobId, data: &[u8]) -> ClientToDaemon {
    ClientToDaemon::SendInput {
        job_id,
        data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
    }
}

/// Build a `DaemonToClient::ToWrapper` input message from raw bytes.
pub fn wrapper_input(data: &[u8]) -> ToWrapper {
    ToWrapper::SendInput {
        data_base64: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data),
    }
}

/// Extract raw bytes from an input message.
pub fn decode_input(data_base64: &str) -> Option<Vec<u8>> {
    base64::Engine::decode(&base64::engine::general_purpose::STANDARD, data_base64).ok()
}

/// Convenience constructor for an error response.
pub fn error_response(code: &str, message: impl Into<String>) -> DaemonToClient {
    DaemonToClient::Error {
        code: code.to_string(),
        message: message.into(),
    }
}

/// Convenience constructor for a job list response.
pub fn job_list(jobs: Vec<JobSummary>) -> DaemonToClient {
    DaemonToClient::JobList { jobs }
}

/// Convenience for an ack.
pub fn ack() -> DaemonToClient {
    DaemonToClient::Ack
}

/// Wrap a `JobState` into the protocol's representation.
pub fn state_to_protocol(state: JobState) -> JobState {
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_round_trip() {
        let data = b"\x01\x02\xff\nbinary";
        let msg = input_message(JobId::default(), data);
        let ClientToDaemon::SendInput { data_base64, .. } = msg else {
            panic!("expected SendInput");
        };
        assert_eq!(decode_input(&data_base64).expect("decode"), data);
    }

    #[test]
    fn job_state_serde_tag() {
        let s = JobState::Exited { code: 42 };
        let json = serde_json::to_value(s).expect("serialize");
        assert_eq!(json["type"], "exited");
        assert_eq!(json["code"], 42);
    }

    #[test]
    fn nested_to_wrapper_round_trips() {
        let msg = DaemonToClient::ToWrapper(ToWrapper::SendInput {
            data_base64: "aGVsbG8=".into(),
        });
        let json = serde_json::to_string(&msg).expect("serialize");
        assert!(json.contains("\"type\":\"to_wrapper\""));
        assert!(json.contains("\"command\":\"send_input\""));
        let back: DaemonToClient = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, msg);
    }
}
