//! Process identifiers and terminal metadata as distinct newtypes.
//!
//! Plain integers are too easy to confuse (child PID vs. process-group ID vs.
//! session ID vs. wrapper PID). These newtypes keep the distinction explicit.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A Unix process id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessId(pub i32);

/// A Unix process-group id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessGroupId(pub i32);

/// A Unix session id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SessionId(pub i32);

/// Terminal window dimensions in characters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WindowSize {
    pub rows: u16,
    pub cols: u16,
    pub x_pixel: u16,
    pub y_pixel: u16,
}

impl WindowSize {
    pub const fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows,
            cols,
            x_pixel: 0,
            y_pixel: 0,
        }
    }
}

impl Default for WindowSize {
    fn default() -> Self {
        Self::new(24, 80)
    }
}

impl fmt::Display for WindowSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}x{}", self.cols, self.rows)
    }
}

/// Metadata about the terminal the job was launched from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct TerminalMetadata {
    /// Whether a controlling terminal was attached at launch.
    pub attached: bool,
    /// The size of the terminal at launch, when available.
    pub initial_size: Option<WindowSize>,
    /// The name of the controlling terminal device, when known.
    pub device: Option<String>,
}
