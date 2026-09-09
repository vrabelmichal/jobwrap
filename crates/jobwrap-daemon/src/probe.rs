//! Restricted help probes: preview, sandboxed execution, and classification.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use jobwrap_protocol::{
    HelpProbeKind, HelpProbeRequest, HelpProbeResult, ProbeClassification, ProbeInvocation,
    ProbePreview,
};

use crate::docs;

/// The environment allow-list for probe execution.
const ENV_ALLOW_LIST: &[&str] = &["LANG", "LC_ALL", "TERM", "TZ"];

/// How long a preview stays valid before execution is refused.
const PREVIEW_TTL: Duration = Duration::from_secs(300);
const MAX_STORED_PREVIEWS: usize = 64;
const MAX_STORED_RESULTS: usize = 32;

/// In-memory stores for probe previews and results.
#[derive(Debug, Default)]
pub struct ProbeStore {
    previews: Mutex<HashMap<String, (HelpProbeRequest, chrono::DateTime<Utc>)>>,
    results: Mutex<HashMap<String, HelpProbeResult>>,
    active: Mutex<usize>,
}

struct ProbePermit<'a>(&'a ProbeStore);

impl Drop for ProbePermit<'_> {
    fn drop(&mut self) {
        let mut active = self.0.active.lock().expect("probe lock");
        *active = active.saturating_sub(1);
    }
}

impl ProbeStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn try_start(&self) -> Option<ProbePermit<'_>> {
        let mut active = self.active.lock().expect("probe lock");
        if *active >= 4 {
            return None;
        }
        *active += 1;
        Some(ProbePermit(self))
    }

    pub fn get_result(&self, probe_id: &str) -> Option<HelpProbeResult> {
        self.results
            .lock()
            .expect("probe lock")
            .get(probe_id)
            .cloned()
    }

    pub fn delete_result(&self, probe_id: &str) -> bool {
        self.results
            .lock()
            .expect("probe lock")
            .remove(probe_id)
            .is_some()
    }

    fn put_preview(&self, id: String, req: HelpProbeRequest) {
        let mut previews = self.previews.lock().expect("probe lock");
        previews.retain(|_, (_, at)| {
            Utc::now().signed_duration_since(*at)
                < chrono::Duration::from_std(PREVIEW_TTL).unwrap_or(chrono::Duration::seconds(300))
        });
        if previews.len() >= MAX_STORED_PREVIEWS {
            if let Some(oldest) = previews
                .iter()
                .min_by_key(|(_, (_, at))| *at)
                .map(|(id, _)| id.clone())
            {
                previews.remove(&oldest);
            }
        }
        previews.insert(id, (req, Utc::now()));
    }

    fn take_preview(&self, id: &str) -> Option<HelpProbeRequest> {
        let mut map = self.previews.lock().expect("probe lock");
        let entry = map.get(id).map(|(_, at)| {
            Utc::now().signed_duration_since(*at)
                < chrono::Duration::from_std(PREVIEW_TTL).unwrap_or(chrono::Duration::seconds(300))
        });
        if entry == Some(false) {
            map.remove(id);
            return None;
        }
        map.remove(id).map(|(req, _)| req)
    }
}

/// Build the exact invocation a probe would run, resolving interpreters and
/// scripts.
pub fn build_invocation(req: &HelpProbeRequest) -> Result<ProbeInvocation, String> {
    let identified = docs::identify(&req.target);
    let probe_arg = req
        .probe_argument
        .clone()
        .unwrap_or_else(|| "--help".into());
    match req.kind {
        HelpProbeKind::Interpreter => {
            let interpreter = identified
                .interpreter
                .ok_or_else(|| "no interpreter could be resolved for this target".to_string())?;
            let mut arguments = Vec::new();
            arguments.extend(probe_arg.split_whitespace().map(str::to_string));
            Ok(ProbeInvocation {
                executable: interpreter,
                arguments,
            })
        }
        HelpProbeKind::Executable | HelpProbeKind::Custom => {
            let mut arguments = Vec::new();
            arguments.push(req.target.clone());
            arguments.extend(probe_arg.split_whitespace().map(str::to_string));
            Ok(ProbeInvocation {
                executable: identified.info.resolved_path,
                arguments,
            })
        }
        HelpProbeKind::Script => {
            let interpreter = identified
                .interpreter
                .ok_or_else(|| "no interpreter could be resolved for this script".to_string())?;
            let mut arguments = Vec::new();
            arguments.extend(identified.interpreter_prefix_arguments.iter().cloned());
            arguments.push(req.target.clone());
            arguments.extend(probe_arg.split_whitespace().map(str::to_string));
            Ok(ProbeInvocation {
                executable: interpreter,
                arguments,
            })
        }
    }
}

/// Create an immutable preview for a probe without executing anything.
pub fn preview_help_probe(
    store: &ProbeStore,
    req: &HelpProbeRequest,
) -> Result<ProbePreview, String> {
    if req.target.is_empty()
        || req.target.len() > 4096
        || req
            .probe_argument
            .as_ref()
            .is_some_and(|argument| argument.len() > 4096 || argument.contains('\0'))
        || req.idempotency_key.is_empty()
        || req.idempotency_key.len() > 128
    {
        return Err("probe target, argument, or idempotency key exceeds safety limits".into());
    }
    let invocation = build_invocation(req)?;
    let executes_target = matches!(
        req.kind,
        HelpProbeKind::Executable | HelpProbeKind::Script | HelpProbeKind::Custom
    );
    let warning = if executes_target {
        "The target may ignore --help or perform normal work.".to_string()
    } else {
        "Interpreter help is probed without executing the target script.".to_string()
    };
    let preview_id = jobwrap_core::JobId::generate().map_err(|e| e.to_string())?;
    let preview = ProbePreview {
        preview_id: preview_id.to_string(),
        invocation,
        executes_target,
        help_support_known: false,
        warning,
    };
    store.put_preview(preview.preview_id.clone(), req.clone());
    Ok(preview)
}

/// Execute a previewed probe in a restricted sandbox.
pub fn execute_help_probe(
    store: &ProbeStore,
    preview_id: &str,
    timeout: Duration,
    output_limit: usize,
) -> Result<HelpProbeResult, String> {
    let _permit = store
        .try_start()
        .ok_or_else(|| "help-probe capacity reached; try again later".to_string())?;
    let req = store
        .take_preview(preview_id)
        .ok_or_else(|| "preview not found or expired".to_string())?;
    let invocation = build_invocation(&req)?;
    let result = run_sandboxed(&invocation, timeout, output_limit);
    let probe_id = jobwrap_core::JobId::generate().map_err(|e| e.to_string())?;
    let result = HelpProbeResult {
        probe_id: probe_id.to_string(),
        ..result
    };
    let mut results = store.results.lock().expect("probe lock");
    if results.len() >= MAX_STORED_RESULTS {
        if let Some(oldest) = results
            .iter()
            .min_by_key(|(_, result)| result.finished_at)
            .map(|(id, _)| id.clone())
        {
            results.remove(&oldest);
        }
    }
    results.insert(result.probe_id.clone(), result.clone());
    Ok(result)
}

/// Run the invocation with limited environment/process controls.
fn run_sandboxed(
    invocation: &ProbeInvocation,
    timeout: Duration,
    output_limit: usize,
) -> HelpProbeResult {
    use std::os::unix::process::CommandExt;

    let started_at = Utc::now();
    let temp_dir = match temp_sandbox_dir() {
        Ok(dir) => dir,
        Err(error) => {
            return HelpProbeResult {
                probe_id: String::new(),
                invocation: invocation.clone(),
                started_at,
                finished_at: Utc::now(),
                exit_code: None,
                timed_out: false,
                output_truncated: false,
                stdout: String::new(),
                stderr: format!("could not create private working directory: {error}"),
                classification: ProbeClassification::ExecutionFailed,
                sandbox_level: "none".into(),
                warning: "The process was not started.".into(),
            };
        }
    };
    let sandbox_home = temp_dir.clone();

    let mut cmd = std::process::Command::new(&invocation.executable);
    cmd.args(&invocation.arguments)
        .current_dir(&temp_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0) // dedicated process group; safe, no unsafe needed.
        .env("HOME", &sandbox_home);
    // Minimal filtered environment.
    cmd.env_clear();
    for var in ENV_ALLOW_LIST {
        if let Ok(value) = std::env::var(var) {
            cmd.env(var, value);
        }
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return HelpProbeResult {
                probe_id: String::new(),
                invocation: invocation.clone(),
                started_at,
                finished_at: Utc::now(),
                exit_code: None,
                timed_out: false,
                output_truncated: false,
                stdout: String::new(),
                stderr: format!("could not spawn: {e}"),
                classification: ProbeClassification::ExecutionFailed,
                sandbox_level: "basic".into(),
                warning: "The process could not be started.".into(),
            };
        }
    };
    let pid = child.id() as i32;
    let stop_readers = Arc::new(AtomicBool::new(false));
    let stdout_reader = child
        .stdout
        .take()
        .and_then(|output| spawn_output_reader(output, output_limit, Arc::clone(&stop_readers)));
    let stderr_reader = child
        .stderr
        .take()
        .and_then(|output| spawn_output_reader(output, output_limit, Arc::clone(&stop_readers)));

    // Kill the child's process group on timeout.
    let started = Instant::now();
    let mut timed_out = false;
    let mut exit_code = None;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = status.code();
                // A short-lived target may leave descendants holding output
                // pipes or doing work. A probe ends the entire dedicated
                // process group even after the direct child exits.
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
                break;
            }
            Ok(None) => {
                if started.elapsed() >= timeout {
                    timed_out = true;
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid),
                        nix::sys::signal::Signal::SIGTERM,
                    );
                    std::thread::sleep(Duration::from_millis(200));
                    let _ = nix::sys::signal::killpg(
                        nix::unistd::Pid::from_raw(pid),
                        nix::sys::signal::Signal::SIGKILL,
                    );
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {
                exit_code = None;
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
                let _ = child.wait();
                break;
            }
        }
    }

    stop_readers.store(true, Ordering::Release);
    let mut stdout = stdout_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let mut stderr = stderr_reader
        .and_then(|reader| reader.join().ok())
        .unwrap_or_default();
    let remaining = output_limit.saturating_sub(stdout.len());
    let output_truncated = stdout.len().saturating_add(stderr.len()) > output_limit;
    if stdout.len() > output_limit {
        stdout.truncate(output_limit);
    }
    if stderr.len() > remaining {
        stderr.truncate(remaining);
    }
    let stdout = String::from_utf8_lossy(&stdout).into_owned();
    let stderr = String::from_utf8_lossy(&stderr).into_owned();
    let _ = std::fs::remove_dir_all(&temp_dir);

    let classification = classify(timed_out, exit_code, &stdout, &stderr);
    let warning = match classification {
        ProbeClassification::TimedOut => "The probe timed out and was terminated.".into(),
        ProbeClassification::LikelyHelpOutput => {
            "The output appears to contain usage documentation.".into()
        }
        ProbeClassification::NormalProgramBehaviorSuspected => {
            "The target may have run normal program logic rather than printing help.".into()
        }
        _ => String::new(),
    };

    HelpProbeResult {
        probe_id: String::new(),
        invocation: invocation.clone(),
        started_at,
        finished_at: Utc::now(),
        exit_code,
        timed_out,
        output_truncated,
        stdout,
        stderr,
        classification,
        sandbox_level: "limited-environment-only".into(),
        warning,
    }
}

fn spawn_output_reader<R>(
    mut output: R,
    output_limit: usize,
    stop: Arc<AtomicBool>,
) -> Option<std::thread::JoinHandle<Vec<u8>>>
where
    R: Read + AsRawFd + Send + 'static,
{
    use nix::fcntl::{fcntl, FcntlArg, OFlag};

    let raw_fd = output.as_raw_fd();
    let flags = fcntl(raw_fd, FcntlArg::F_GETFL).ok()?;
    fcntl(
        raw_fd,
        FcntlArg::F_SETFL(OFlag::from_bits_truncate(flags) | OFlag::O_NONBLOCK),
    )
    .ok()?;

    std::thread::Builder::new()
        .name("jobwrap-probe-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let mut buffer = [0u8; 8192];
            loop {
                match output.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        let remaining = output_limit.saturating_add(1).saturating_sub(bytes.len());
                        bytes.extend_from_slice(&buffer[..count.min(remaining)]);
                        if bytes.len() > output_limit {
                            break;
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if stop.load(Ordering::Acquire) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
            bytes
        })
        .ok()
}

fn classify(
    timed_out: bool,
    exit_code: Option<i32>,
    stdout: &str,
    stderr: &str,
) -> ProbeClassification {
    if timed_out {
        return ProbeClassification::TimedOut;
    }
    let output = format!("{stdout}\n{stderr}").to_lowercase();
    let has_help_markers = [
        "usage:",
        "usage ",
        "synopsis",
        "options:",
        "available options",
        "--help",
        "-h ",
        "commands:",
    ]
    .iter()
    .any(|m| output.contains(m));
    if stderr.to_lowercase().contains("unknown option")
        || stderr.to_lowercase().contains("invalid option")
        || stderr.to_lowercase().contains("unrecognized option")
    {
        return ProbeClassification::ArgumentRejected;
    }
    match exit_code {
        Some(0) if has_help_markers => ProbeClassification::LikelyHelpOutput,
        Some(0) if !stdout.is_empty() && stdout.len() > 2000 => {
            ProbeClassification::NormalProgramBehaviorSuspected
        }
        Some(0) if stdout.is_empty() && stderr.is_empty() => ProbeClassification::NoOutput,
        Some(0) => ProbeClassification::PossibleHelpOutput,
        Some(_) if has_help_markers => ProbeClassification::PossibleHelpOutput,
        Some(_) if stdout.is_empty() && stderr.is_empty() => ProbeClassification::NoOutput,
        Some(_) => ProbeClassification::ExecutionFailed,
        None if stdout.is_empty() && stderr.is_empty() => ProbeClassification::NoOutput,
        None => ProbeClassification::Unknown,
    }
}

/// Create a private temporary sandbox directory.
fn temp_sandbox_dir() -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;

    let base = std::env::temp_dir();
    let id = jobwrap_core::JobId::generate()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error))?;
    let dir = base.join(format!("jobwrap-probe-{id}"));
    std::fs::create_dir(&dir)?;
    if let Err(error) = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)) {
        let _ = std::fs::remove_dir(&dir);
        return Err(error);
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_help_output() {
        let c = classify(
            false,
            Some(0),
            "Usage: grep [OPTION]... PATTERNS [FILE]...",
            "",
        );
        assert_eq!(c, ProbeClassification::LikelyHelpOutput);
    }

    #[test]
    fn does_not_assume_help_from_exit_zero_alone() {
        // Exit zero alone is not proof of help support; short ambiguous output
        // is classified as possible help output, never "definitely safe".
        let c = classify(false, Some(0), "processing 1000 rows... done", "");
        assert_eq!(c, ProbeClassification::PossibleHelpOutput);
    }

    #[test]
    fn argument_rejected_detected() {
        let c = classify(false, Some(1), "", "error: unrecognized option '--help'");
        assert_eq!(c, ProbeClassification::ArgumentRejected);
    }

    #[test]
    fn timeout_detected() {
        let c = classify(true, None, "", "");
        assert_eq!(c, ProbeClassification::TimedOut);
    }

    #[test]
    fn no_output_classification() {
        assert_eq!(
            classify(false, Some(0), "", ""),
            ProbeClassification::NoOutput
        );
        assert_eq!(
            classify(false, Some(1), "", ""),
            ProbeClassification::NoOutput
        );
    }

    #[test]
    fn unknown_when_killed_without_output() {
        assert_eq!(classify(false, None, "", ""), ProbeClassification::NoOutput);
    }

    #[test]
    fn output_reader_can_stop_with_an_open_writer() {
        let (reader, _writer) = std::os::unix::net::UnixStream::pair().expect("socket pair");
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn_output_reader(reader, 1024, Arc::clone(&stop)).expect("reader thread");
        stop.store(true, Ordering::Release);
        assert!(handle.join().expect("reader join").is_empty());
    }

    #[test]
    fn interpreter_invocation_does_not_execute_target() {
        // An interpreter probe must resolve the interpreter from a shebang and
        // probe only the interpreter, never run the script.
        let dir = std::env::temp_dir().join(format!("jw-probe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let script = dir.join("probe_script.py");
        std::fs::write(&script, "#!/usr/bin/env python3\nprint('x')\n").expect("write");
        let req = HelpProbeRequest {
            target: script.display().to_string(),
            probe_argument: Some("--help".into()),
            kind: HelpProbeKind::Interpreter,
            idempotency_key: "k".into(),
        };
        let inv = build_invocation(&req).expect("invocation");
        assert!(
            inv.executable.contains("python"),
            "expected python interpreter, got {}",
            inv.executable
        );
        assert_eq!(inv.arguments, vec!["--help".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn script_invocation_uses_interpreter_and_target() {
        let req = HelpProbeRequest {
            target: "/usr/bin/python3".into(),
            probe_argument: Some("--help".into()),
            kind: HelpProbeKind::Executable,
            idempotency_key: "k".into(),
        };
        let inv = build_invocation(&req).expect("invocation");
        assert!(inv.arguments.contains(&"/usr/bin/python3".to_string()));
        assert!(inv.arguments.contains(&"--help".to_string()));
    }
}
