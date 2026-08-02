//! Protocol errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("frame exceeds the maximum allowed size of {size} bytes")]
    FrameTooLarge { size: usize },
    #[error("protocol version {found} is unsupported; this daemon speaks version {supported}")]
    VersionMismatch { found: u32, supported: u32 },
}
