//! Static documentation discovery: target identification, man-page lookup, and
//! package metadata. None of these functions execute the target.

use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use jobwrap_protocol::{ManPage, ManPageMatch, TargetInfo};

static DOC_COMMAND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Static information about a target file.
#[derive(Debug, Clone)]
pub struct Identified {
    pub info: TargetInfo,
    /// The parsed interpreter (resolved through PATH when the shebang uses
    /// `/usr/bin/env`).
    pub interpreter: Option<String>,
    pub interpreter_prefix_arguments: Vec<String>,
}

/// Fingerprint a file using path, size and mtime; the caller should also
/// include the probe arguments when deriving a help-probe cache key.
pub fn fingerprint(path: &Path, size: u64, mtime_nanos: i64) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(path.as_os_str().as_bytes());
    hasher.update(size.to_le_bytes());
    hasher.update(mtime_nanos.to_le_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Build a help-probe cache key from the target fingerprint, resolved path,
/// and probe arguments.
pub fn cache_key(resolved_path: &str, target_fingerprint: &str, probe_arguments: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(resolved_path.as_bytes());
    hasher.update(target_fingerprint.as_bytes());
    hasher.update(probe_arguments.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Identify a target without executing it.
pub fn identify(target: &str) -> Identified {
    let original = target.to_string();
    let target_path = resolve_target_path(target);
    let path = target_path.as_path();
    let (resolved, exists) = match std::fs::canonicalize(path) {
        Ok(p) => (p, true),
        Err(_) => (path.to_path_buf(), false),
    };
    let mut info = TargetInfo {
        original_path: original,
        resolved_path: resolved.display().to_string(),
        exists,
        file_type: None,
        executable: false,
        shebang: None,
        interpreter: None,
        interpreter_prefix_arguments: Vec::new(),
        target_kind: None,
        mtime_nanos: None,
        fingerprint: None,
        size_bytes: None,
        package: None,
    };
    if !exists {
        return Identified {
            info,
            interpreter: None,
            interpreter_prefix_arguments: Vec::new(),
        };
    }

    let meta = std::fs::metadata(&resolved);
    if let Ok(meta) = meta {
        info.executable = meta.permissions().mode() & 0o111 != 0;
        info.size_bytes = Some(meta.len());
        info.mtime_nanos = Some(
            meta.mtime()
                .saturating_mul(1_000_000_000)
                .saturating_add(meta.mtime_nsec()),
        );
        info.file_type = Some(file_type_label(&meta));
        let total_nanos = meta
            .mtime()
            .saturating_mul(1_000_000_000)
            .saturating_add(meta.mtime_nsec());
        info.fingerprint = Some(fingerprint(&resolved, meta.len(), total_nanos));
    }

    // Inspect only a small prefix of regular files. Reading the whole target
    // would be wasteful for large binaries, and opening FIFOs/devices could
    // block the daemon indefinitely.
    if info.file_type.as_deref() == Some("regular") {
        if let Some(first) = read_prefix(&resolved, 8 * 1024) {
            if let Some(shebang) = first
                .lines()
                .next()
                .and_then(|line| line.strip_prefix("#!"))
            {
                info.shebang = Some(shebang.trim().to_string());
            }
        }
    }

    // Resolve the interpreter.
    let (interpreter, prefix) = resolve_interpreter(&info);
    info.interpreter = interpreter.clone();
    info.interpreter_prefix_arguments = prefix.clone();
    info.target_kind = target_kind(&info);

    Identified {
        info,
        interpreter,
        interpreter_prefix_arguments: prefix,
    }
}

fn resolve_target_path(target: &str) -> PathBuf {
    if target.contains('/') {
        return PathBuf::from(target);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|dir| dir.join(target))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| PathBuf::from(target))
}

fn read_prefix(path: &Path, limit: u64) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(limit).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn file_type_label(meta: &std::fs::Metadata) -> String {
    use std::os::unix::fs::FileTypeExt;
    let ft = meta.file_type();
    if ft.is_dir() {
        "directory".into()
    } else if ft.is_symlink() {
        "symlink".into()
    } else if ft.is_block_device() {
        "block_device".into()
    } else if ft.is_char_device() {
        "char_device".into()
    } else if ft.is_fifo() {
        "fifo".into()
    } else if ft.is_socket() {
        "socket".into()
    } else {
        "regular".into()
    }
}

fn target_kind(info: &TargetInfo) -> Option<String> {
    if !info.executable {
        return Some("non_executable".into());
    }
    match (info.file_type.as_deref(), info.shebang.as_deref()) {
        (Some("regular"), Some(_)) => Some("script".into()),
        (Some("regular"), None) => Some("binary".into()),
        _ => Some("other".into()),
    }
}

/// Resolve the interpreter named in a shebang, following `/usr/bin/env`.
fn resolve_interpreter(info: &TargetInfo) -> (Option<String>, Vec<String>) {
    let Some(shebang) = &info.shebang else {
        return (None, Vec::new());
    };
    let mut parts = shebang.split_whitespace();
    let first = parts.next().unwrap_or_default();
    let rest: Vec<String> = parts.map(str::to_string).collect();
    if first == "/usr/bin/env" {
        let Some(prog) = rest.first() else {
            return (None, rest);
        };
        // Resolve through PATH, defaulting to the plain name.
        let resolved = resolve_in_path(prog);
        return (Some(resolved), rest[1..].to_vec());
    }
    (Some(first.to_string()), rest)
}

fn resolve_in_path(prog: &str) -> String {
    if prog.contains('/') {
        return prog.to_string();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(prog);
        if candidate.is_file() {
            return candidate.display().to_string();
        }
    }
    prog.to_string()
}

/// Search man pages for a target without executing the target.
pub fn search_man_pages(target: &str) -> Vec<ManPageMatch> {
    let identified = identify(target);
    let mut names: Vec<(String, String)> = Vec::new(); // (name, relationship)
    if let Some(base) = Path::new(&identified.info.resolved_path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
    {
        names.push((base.clone(), "resolved-executable".into()));
    }
    if let Some(pkg) = &identified.info.package {
        names.push((pkg.clone(), "package".into()));
    }
    if let Some(interp) = &identified.info.interpreter {
        if let Some(base) = Path::new(interp)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
        {
            if !names.iter().any(|(n, _)| n == &base) {
                names.push((base, "interpreter".into()));
            }
        }
    }

    let mut matches = Vec::new();
    for (name, relationship) in names {
        let found = find_man_page(&name, None);
        matches.push(ManPageMatch {
            name,
            section: found
                .as_ref()
                .map(|(s, _)| s.to_string())
                .unwrap_or_default(),
            relationship,
            content_available: found.is_some(),
        });
    }
    matches
}

/// Find the (section, path) for a man page, optionally in a given section.
fn find_man_page(name: &str, section: Option<&str>) -> Option<(String, String)> {
    if !valid_man_component(name) || section.is_some_and(|s| !valid_man_component(s)) {
        return None;
    }
    let mut cmd = Command::new("man");
    cmd.arg("-w");
    if let Some(section) = section {
        cmd.arg("-s").arg(section);
    }
    cmd.arg("--").arg(name);
    let out = bounded_output(&mut cmd, Duration::from_secs(2), 64 * 1024)?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if text.is_empty() {
        return None;
    }
    // Section is the first part of the basename before the first dot.
    let base = Path::new(&text)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let section = base
        .split('.')
        .nth(1)
        .map(str::to_string)
        .unwrap_or_else(|| section.unwrap_or("1").to_string());
    Some((section, text))
}

/// Fetch formatted man-page content.
pub fn fetch_man_page(name: &str, section: Option<&str>) -> Option<ManPage> {
    if !valid_man_component(name) || section.is_some_and(|s| !valid_man_component(s)) {
        return None;
    }
    let mut cmd = Command::new("man");
    cmd.arg("-P").arg("cat");
    if let Some(section) = section {
        cmd.arg("-s").arg(section);
    }
    cmd.arg("--").arg(name);
    let out = bounded_output(&mut cmd, Duration::from_secs(3), 2 * 1024 * 1024)?;
    if !out.status.success() {
        return None;
    }
    let content = String::from_utf8_lossy(&out.stdout).into_owned();
    let resolved_section = find_man_page(name, section)
        .map(|(s, _)| s)
        .unwrap_or_else(|| section.unwrap_or("1").to_string());
    Some(ManPage {
        name: name.to_string(),
        section: resolved_section,
        content: Some(content),
    })
}

fn valid_man_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'_' | b'.'))
        && !value.starts_with('-')
}

struct BoundedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
}

/// Run a metadata helper with fixed time and output limits. Helpers get a
/// dedicated process group so a timeout also terminates descendants.
fn bounded_output(cmd: &mut Command, timeout: Duration, max_bytes: u64) -> Option<BoundedOutput> {
    use std::os::unix::process::CommandExt;

    let _permit = DOC_COMMAND_LOCK.try_lock().ok()?;

    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0);
    let mut child = cmd.spawn().ok()?;
    let pid = child.id() as i32;
    let stdout = child.stdout.take()?;
    let reader = std::thread::Builder::new()
        .name("jw-doc-reader".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let _ = stdout.take(max_bytes).read_to_end(&mut bytes);
            bytes
        })
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < timeout => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                let _ = nix::sys::signal::killpg(
                    nix::unistd::Pid::from_raw(pid),
                    nix::sys::signal::Signal::SIGKILL,
                );
                break child.wait().ok()?;
            }
            Err(_) => return None,
        }
    };
    let stdout = reader.join().ok()?;
    Some(BoundedOutput { status, stdout })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_bare_commands_from_path() {
        let info = identify("sh").info;
        assert!(info.exists);
        assert!(Path::new(&info.resolved_path).is_absolute());
    }

    #[test]
    fn rejects_man_option_injection() {
        assert!(!valid_man_component("--html=evil"));
        assert!(valid_man_component("printf"));
        assert!(valid_man_component("3p"));
    }
}
