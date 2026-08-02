//! Typed wire protocol between wrapper, daemon, and CLI.
//!
//! Frames are length-prefixed JSON. Binary payloads (terminal output, input)
//! are carried as base64 strings so the JSON stream is always valid UTF-8 and
//! never contains control bytes.
//!
//! The daemon distinguishes a wrapper connection from a CLI connection by the
//! [`ClientToDaemon::Hello`] role.
#![forbid(unsafe_code)]
#![forbid(unsafe_code)]

pub mod codec;
pub mod message;
pub mod types;

pub use codec::{decode_frame, encode_frame, FrameDecoder, PROTOCOL_HEADER_BYTES};
pub use error::ProtocolError;
pub use message::{
    decode_input, error_response, input_message, wrapper_input, ClientToDaemon, DaemonToClient,
    Hello, HelloRole, RegisterJob, RegisterResult, ToWrapper, TokenCreateRequest, WrapperToDaemon,
    PROTOCOL_VERSION,
};
pub use types::{ApiErrorCode, DaemonError, JobSummary, OutputRange, OutputSlice, TokenInfo};

mod error;
