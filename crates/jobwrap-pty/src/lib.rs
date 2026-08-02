//! jobwrap-pty: Linux process and terminal logic.
//!
//! This crate owns all low-level operating-system interaction: PTY allocation,
//! process groups, signal forwarding, terminal-mode guards, and the relay loop.
//! All `unsafe` is isolated in [`ffi`].

#![deny(unsafe_op_in_unsafe_fn)]
#![allow(clippy::module_name_repetitions)]

pub mod child;
pub mod error;
pub mod ffi;
pub mod pty;
pub mod relay;
pub mod terminal;

pub use child::{ChildStateChange, ChildStatus, SpawnedChild};
pub use error::PtyError;
pub use pty::PseudoTerminal;
pub use relay::{command_channel, run_relay, DaemonCommand, RelaySinks};
pub use terminal::{RelayModeGuard, SavedTerminal, SlaveTermios, TerminalGuard};
