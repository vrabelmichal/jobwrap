//! Named signals that can be delivered to a process group.
//!
//! The API deliberately exposes a small allow-list of named signals rather
//! than arbitrary numeric signals.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The signals the system may deliver to a job's process group.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Signal {
    Interrupt,
    Terminate,
    Hangup,
    Quit,
    Stop,
    Continue,
    Kill,
}

/// Error produced when an unknown signal name is parsed.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unknown signal `{name}`; expected one of: int, term, hup, quit, stop, cont, kill")]
pub struct SignalParseError {
    name: String,
}

impl Signal {
    /// The full signal name.
    pub fn name(self) -> &'static str {
        match self {
            Signal::Interrupt => "SIGINT",
            Signal::Terminate => "SIGTERM",
            Signal::Hangup => "SIGHUP",
            Signal::Quit => "SIGQUIT",
            Signal::Stop => "SIGSTOP",
            Signal::Continue => "SIGCONT",
            Signal::Kill => "SIGKILL",
        }
    }

    /// The short name used on the CLI and API.
    pub fn short(self) -> &'static str {
        match self {
            Signal::Interrupt => "int",
            Signal::Terminate => "term",
            Signal::Hangup => "hup",
            Signal::Quit => "quit",
            Signal::Stop => "stop",
            Signal::Continue => "cont",
            Signal::Kill => "kill",
        }
    }
}

impl FromStr for Signal {
    type Err = SignalParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_ascii_uppercase();
        let name = normalized.trim_start_matches("SIG");
        let parsed = match name {
            "INT" | "INTERRUPT" => Signal::Interrupt,
            "TERM" | "TERMINATE" => Signal::Terminate,
            "HUP" | "HANGUP" => Signal::Hangup,
            "QUIT" => Signal::Quit,
            "STOP" | "TSTP" => Signal::Stop,
            "CONT" | "CONTINUE" => Signal::Continue,
            "KILL" => Signal::Kill,
            _ => {
                return Err(SignalParseError {
                    name: s.to_string(),
                })
            }
        };
        Ok(parsed)
    }
}

impl fmt::Display for Signal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_forms() {
        for raw in ["int", "INT", "SIGINT", "interrupt"] {
            assert_eq!(raw.parse::<Signal>().expect("parses"), Signal::Interrupt);
        }
        for raw in ["term", "TERM", "SIGTERM"] {
            assert_eq!(raw.parse::<Signal>().expect("parses"), Signal::Terminate);
        }
        assert_eq!("kill".parse::<Signal>().expect("parses"), Signal::Kill);
        assert_eq!("cont".parse::<Signal>().expect("parses"), Signal::Continue);
    }

    #[test]
    fn rejects_unknown() {
        assert!("SIGUSR1".parse::<Signal>().is_err());
        assert!("garbage".parse::<Signal>().is_err());
    }
}
