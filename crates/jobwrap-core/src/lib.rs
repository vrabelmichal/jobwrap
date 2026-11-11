//! jobwrap-core: domain types, state machines, and authorization primitives.
//!
//! This crate has no HTTP or database code. It defines the vocabulary the rest
//! of the workspace is built on:
//!
//! * [`JobId`] — a ULID identifying a job;
//! * [`JobState`] — an explicit, centrally validated state machine;
//! * [`Permission`], [`RequiredAccess`], [`AccessPolicy`] and [`Principal`] —
//!   the authorization vocabulary;
//! * process identifiers as distinct newtypes;
//! * structured [`Event`]s and [`JobRecord`]s.

#![forbid(unsafe_code)]

mod access;
pub mod build_info;
mod error;
mod event;
pub mod execution;
mod job;
mod job_id;
mod launch;
mod process;
mod signal;
mod state;

pub use access::{
    authorize, authorize_global, AccessLevel, AccessPolicy, AuthorizationDecision,
    GlobalPermission, Permission, Principal, ProfileAccess, RequiredAccess, ResourceSelector,
    TokenGrant,
};
pub use error::CoreError;
pub use event::{Event, EventId, EventKind};
pub use job::{detached_service_unit, CommandDisplay, JobName, JobRecord};
pub use job_id::{JobId, JobIdError, ULID_EPOCH};
pub use launch::{LaunchMode, TerminalTarget};
pub use process::{ProcessGroupId, ProcessId, SessionId, TerminalMetadata, WindowSize};
pub use signal::{Signal, SignalParseError};
pub use state::{JobState, StateTransitionError};

/// Identity survives reconnects but not reboot or PID reuse.
pub fn worker_identity(pid: i32) -> std::io::Result<String> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let tail = stat
        .rsplit_once(") ")
        .ok_or_else(|| other_error("invalid process stat"))?
        .1;
    let start = tail
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| other_error("missing process start"))?;
    Ok(format!("{}:{start}", boot.trim()))
}

fn other_error(e: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::Other, e)
}
