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
pub use job::{CommandDisplay, JobName, JobRecord};
pub use job_id::{JobId, JobIdError, ULID_EPOCH};
pub use launch::{LaunchMode, TerminalTarget};
pub use process::{ProcessGroupId, ProcessId, SessionId, TerminalMetadata, WindowSize};
pub use signal::{Signal, SignalParseError};
pub use state::{JobState, StateTransitionError};
