//! Terminal backend abstraction and the existing-terminal registry.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use chrono::{DateTime, Utc};
use jobwrap_protocol::TerminalInfo;

/// The command the daemon asks a terminal emulator to run.
#[derive(Debug, Clone)]
pub struct HelperCommand {
    /// The one-time launch id.
    pub launch_id: String,
    /// The absolute path to the `jobwrap` helper binary.
    pub helper_binary: String,
}

/// A terminal emulator backend.
pub trait TerminalBackend: Send + Sync {
    /// Stable identifier such as `gnome-terminal`.
    fn identifier(&self) -> &'static str;

    /// Whether the emulator binary is present.
    fn available(&self) -> bool;

    /// Launch a new terminal running `helper`; the emulator process is spawned
    /// detached and we return immediately.
    fn launch(&self, helper: &HelperCommand) -> Result<(), String>;
}

/// GNOME Terminal backend.
#[derive(Debug, Default)]
pub struct GnomeTerminalBackend;

impl TerminalBackend for GnomeTerminalBackend {
    fn identifier(&self) -> &'static str {
        "gnome-terminal"
    }

    fn available(&self) -> bool {
        which("gnome-terminal")
    }

    fn launch(&self, helper: &HelperCommand) -> Result<(), String> {
        let mut cmd = std::process::Command::new("gnome-terminal");
        cmd.arg("--")
            .arg(&helper.helper_binary)
            .arg("attach-launch")
            .arg(&helper.launch_id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        spawn_detached(cmd)
    }
}

/// xterm-compatible fallback backend.
#[derive(Debug, Default)]
pub struct XtermBackend;

impl TerminalBackend for XtermBackend {
    fn identifier(&self) -> &'static str {
        "xterm"
    }

    fn available(&self) -> bool {
        which("xterm")
    }

    fn launch(&self, helper: &HelperCommand) -> Result<(), String> {
        let mut cmd = std::process::Command::new("xterm");
        cmd.arg("-e")
            .arg(&helper.helper_binary)
            .arg("attach-launch")
            .arg(&helper.launch_id)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        spawn_detached(cmd)
    }
}

fn which(binary: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        if dir.join(binary).is_file() {
            return true;
        }
    }
    false
}

fn spawn_detached(mut cmd: std::process::Command) -> Result<(), String> {
    cmd.spawn()
        .map(|_| ())
        .map_err(|e| format!("could not start terminal: {e}"))
}

/// Resolve a backend by identifier, falling back to the configured preferred
/// backend.
pub fn resolve_backend(identifier: &str) -> Box<dyn TerminalBackend> {
    match identifier {
        "gnome-terminal" => Box::<GnomeTerminalBackend>::default(),
        "xterm" => Box::<XtermBackend>::default(),
        other => {
            tracing::warn!(backend = %other, "unknown terminal backend; falling back to gnome-terminal");
            Box::<GnomeTerminalBackend>::default()
        }
    }
}

/// A terminal registered with jobwrap that can accept a cooperative launch.
#[derive(Debug, Clone)]
pub struct RegisteredTerminal {
    pub terminal_id: String,
    pub owner_uid: u32,
    pub state: TerminalState,
    pub shell_type: Option<String>,
    pub working_directory: Option<String>,
    /// FIFO path the cooperative shell hook reads structured commands from.
    pub control_path: Option<String>,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

/// The state of a registered terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalState {
    Ready,
    Busy,
    Disconnected,
}

impl TerminalState {
    pub fn as_str(self) -> &'static str {
        match self {
            TerminalState::Ready => "ready",
            TerminalState::Busy => "busy",
            TerminalState::Disconnected => "disconnected",
        }
    }
}

/// In-memory registry of jobwrap-registered terminals.
#[derive(Debug, Default)]
pub struct TerminalRegistry {
    terminals: Mutex<HashMap<String, RegisteredTerminal>>,
}

/// How long after the last heartbeat a terminal is considered disconnected.
const HEARTBEAT_TTL: Duration = Duration::from_secs(90);

impl TerminalRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, terminal: RegisteredTerminal) {
        let mut map = self.terminals.lock().expect("terminal lock");
        map.insert(terminal.terminal_id.clone(), terminal);
    }

    #[allow(dead_code)] // wired by the Phase 9 shell hook
    pub fn heartbeat(&self, terminal_id: &str, cwd: Option<&str>) -> bool {
        let mut map = self.terminals.lock().expect("terminal lock");
        if let Some(t) = map.get_mut(terminal_id) {
            t.last_heartbeat = Utc::now();
            if let Some(cwd) = cwd {
                t.working_directory = Some(cwd.to_string());
            }
            true
        } else {
            false
        }
    }

    pub fn get(&self, terminal_id: &str) -> Option<RegisteredTerminal> {
        let map = self.terminals.lock().expect("terminal lock");
        map.get(terminal_id).map(|t| prune(t, Utc::now()).clone())
    }

    pub fn list(&self) -> Vec<RegisteredTerminal> {
        let now = Utc::now();
        let map = self.terminals.lock().expect("terminal lock");
        map.values().map(|t| prune(t, now).clone()).collect()
    }

    /// Whether the terminal is currently Ready (fresh heartbeat, state Ready).
    #[allow(dead_code)] // wired by the Phase 9 shell hook
    pub fn is_ready(&self, terminal_id: &str) -> bool {
        self.get(terminal_id)
            .map(|t| t.state == TerminalState::Ready)
            .unwrap_or(false)
    }
}

/// Mark stale terminals as disconnected and return the (possibly updated)
/// terminal.
fn prune(t: &RegisteredTerminal, now: DateTime<Utc>) -> RegisteredTerminal {
    let mut out = t.clone();
    if now.signed_duration_since(out.last_heartbeat)
        > chrono::Duration::from_std(HEARTBEAT_TTL)
            .unwrap_or(chrono::Duration::from_std(Duration::from_secs(90)).unwrap())
    {
        out.state = TerminalState::Disconnected;
    }
    out
}

/// Convert a registered terminal into the wire info type.
pub fn to_info(t: &RegisteredTerminal) -> TerminalInfo {
    TerminalInfo {
        terminal_id: t.terminal_id.clone(),
        owner_uid: t.owner_uid,
        state: t.state.as_str().to_string(),
        shell_type: t.shell_type.clone(),
        working_directory: t.working_directory.clone(),
        registered_at: t.registered_at,
        last_heartbeat: t.last_heartbeat,
    }
}
