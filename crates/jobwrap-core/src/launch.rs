//! Launch domain types: terminal targets and launch modes.

use serde::{Deserialize, Serialize};

/// Where a launched process should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    /// A process owned by jobwrap with no graphical terminal.
    Managed,
    /// A new graphical terminal window/pane running the managed command.
    NewTerminal,
    /// A command started inside an existing jobwrap-registered terminal.
    ExistingTerminal,
}

impl LaunchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            LaunchMode::Managed => "managed",
            LaunchMode::NewTerminal => "new_terminal",
            LaunchMode::ExistingTerminal => "existing_terminal",
        }
    }
}

impl std::fmt::Display for LaunchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The requested terminal target of a launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TerminalTarget {
    /// No graphical terminal; the process is owned by jobwrap.
    #[default]
    Managed,
    /// Open a new terminal using the given backend identifier (optional; falls
    /// back to the configured preferred backend).
    NewTerminal {
        /// A configured backend identifier such as `gnome-terminal`.
        backend: Option<String>,
    },
    /// Start the command inside an existing registered terminal.
    ExistingTerminal { terminal_id: String },
}

impl TerminalTarget {
    /// The launch mode this target corresponds to.
    pub fn mode(&self) -> LaunchMode {
        match self {
            TerminalTarget::Managed => LaunchMode::Managed,
            TerminalTarget::NewTerminal { .. } => LaunchMode::NewTerminal,
            TerminalTarget::ExistingTerminal { .. } => LaunchMode::ExistingTerminal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trip() {
        let target = TerminalTarget::NewTerminal {
            backend: Some("gnome-terminal".into()),
        };
        let json = serde_json::to_string(&target).expect("serialize");
        assert!(json.contains("new_terminal"));
        let back: TerminalTarget = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, target);
    }

    #[test]
    fn modes() {
        assert_eq!(TerminalTarget::Managed.mode(), LaunchMode::Managed);
        assert_eq!(
            TerminalTarget::NewTerminal { backend: None }.mode(),
            LaunchMode::NewTerminal
        );
        assert_eq!(
            TerminalTarget::ExistingTerminal {
                terminal_id: "t".into()
            }
            .mode(),
            LaunchMode::ExistingTerminal
        );
    }
}
