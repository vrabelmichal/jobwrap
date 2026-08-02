//! PTY errors.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum PtyError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}
