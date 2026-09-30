//! Rich, permission-aware job details for the browser interface.
//!
//! The regular job record API intentionally exposes only status-level fields.
//! This endpoint adds command/working-directory fields only when the current
//! principal has the corresponding permissions, and augments live jobs with a
//! small, curated `/proc` snapshot.

use std::fs::{self, File};
use std::io::{self, Read};
use std::path::Path as FsPath;

use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use serde::Serialize;
use serde_json::{json, Value};

use jobwrap_core::{
    authorize, AuthorizationDecision, JobId, JobRecord, JobState, Permission, Principal,
};

use crate::error::ApiError;
use crate::router::RouterState;

const SESSION_COOKIE: &str = "jobwrap_session";
const MAX_PROC_TEXT_BYTES: usize = 64 * 1024;

#[derive(Debug, Serialize)]
struct ProcessSnapshot {
    pid: i32,
    state: Option<String>,
    parent_pid: Option<i32>,
    threads: Option<u64>,
    resident_memory_bytes: Option<u64>,
    virtual_memory_bytes: Option<u64>,
    open_file_descriptors: Option<u64>,
    read_bytes: Option<u64>,
    write_bytes: Option<u64>,
    current_command: Option<String>,
    current_executable: Option<String>,
    current_working_directory: Option<String>,
}

pub(crate) async fn get_job_details(
    State(state): State<RouterState>,
    Path(id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let id = parse_job_id(&id)?;
    let principal = principal_from_headers(&state, &headers);
    let record = state.service.get_job(&principal, id)?;
    Ok(Json(details_response(&record, &principal)))
}

fn details_response(record: &JobRecord, principal: &Principal) -> Value {
    let view_output = allowed(principal, record, Permission::ViewOutput);
    let view_command = allowed(principal, record, Permission::ViewCommand);
    let view_working_directory = allowed(principal, record, Permission::ViewWorkingDirectory);

    let command = view_command.then(|| record.command.as_str().to_string());
    let executable = view_command.then(|| record.executable.to_string_lossy().into_owned());
    let arguments = view_command.then(|| record.arguments.clone());
    let working_directory =
        view_working_directory.then(|| record.working_directory.to_string_lossy().into_owned());
    let process = process_snapshot(record, view_command, view_working_directory);

    json!({
        "id": record.id.to_string(),
        "name": record.display_name.as_str(),
        "state": record.state,
        "profile": record.profile_name,
        "started_at": record.started_at.to_rfc3339(),
        "finished_at": record.finished_at.map(|time| time.to_rfc3339()),
        "wrapper_pid": record.wrapper_pid.map(|pid| pid.0),
        "child_pid": record.child_pid.map(|pid| pid.0),
        "process_group_id": record.process_group_id.map(|pgid| pgid.0),
        "command": command,
        "executable": executable,
        "arguments": arguments,
        "working_directory": working_directory,
        "terminal": {
            "attached": record.terminal.attached,
            "device": record.terminal.device.as_deref(),
            "initial_size": record.terminal.initial_size,
        },
        "log": {
            "bytes_written": record.log.bytes_written,
            "last_sequence": record.log.last_sequence,
            "truncated": record.log.truncated,
        },
        "process": process,
        "permissions": {
            "view_output": view_output,
            "view_command": view_command,
            "view_working_directory": view_working_directory,
            "send_interrupt": allowed(principal, record, Permission::SendInterrupt),
            "send_terminate": allowed(principal, record, Permission::SendTerminate),
            "send_stop": allowed(principal, record, Permission::SendStop),
            "send_continue": allowed(principal, record, Permission::SendContinue),
            "send_kill": allowed(principal, record, Permission::SendKill),
        },
    })
}

fn allowed(principal: &Principal, record: &JobRecord, permission: Permission) -> bool {
    matches!(
        authorize(principal, record, permission),
        AuthorizationDecision::Allow
    )
}

fn process_snapshot(
    record: &JobRecord,
    include_command: bool,
    include_working_directory: bool,
) -> Option<ProcessSnapshot> {
    // Stored PIDs must not be trusted after the wrapper loses authoritative
    // contact with the job: Linux can reuse a PID for an unrelated process.
    if !matches!(&record.state, JobState::Running | JobState::Stopped) {
        return None;
    }

    let pid = record.child_pid?.0;
    let base = format!("/proc/{pid}");
    let status = fs::read_to_string(format!("{base}/status")).ok()?;
    let io_text = fs::read_to_string(format!("{base}/io")).ok();

    let open_file_descriptors = fs::read_dir(format!("{base}/fd"))
        .ok()
        .and_then(|entries| u64::try_from(entries.filter_map(Result::ok).count()).ok());

    let cmdline_path = format!("{base}/cmdline");
    let current_command = include_command
        .then(|| read_cmdline(FsPath::new(&cmdline_path)))
        .flatten();
    let current_executable = include_command
        .then(|| fs::read_link(format!("{base}/exe")).ok())
        .flatten()
        .map(|path| path.to_string_lossy().into_owned());
    let current_working_directory = include_working_directory
        .then(|| fs::read_link(format!("{base}/cwd")).ok())
        .flatten()
        .map(|path| path.to_string_lossy().into_owned());

    Some(ProcessSnapshot {
        pid,
        state: status_field(&status, "State").map(str::to_string),
        parent_pid: status_field(&status, "PPid").and_then(parse_i32),
        threads: status_field(&status, "Threads").and_then(parse_u64),
        resident_memory_bytes: status_field(&status, "VmRSS").and_then(parse_kibibytes),
        virtual_memory_bytes: status_field(&status, "VmSize").and_then(parse_kibibytes),
        open_file_descriptors,
        read_bytes: io_text
            .as_deref()
            .and_then(|text| named_counter(text, "read_bytes")),
        write_bytes: io_text
            .as_deref()
            .and_then(|text| named_counter(text, "write_bytes")),
        current_command,
        current_executable,
        current_working_directory,
    })
}

fn principal_from_headers(state: &RouterState, headers: &HeaderMap) -> Principal {
    let session = cookie_value(headers, SESSION_COOKIE);
    let bearer = bearer_token(headers);
    state
        .service
        .resolve_principal(session.as_deref(), bearer.as_deref())
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookie = headers.get("cookie")?.to_str().ok()?;
    cookie.split(';').find_map(|part| {
        part.trim()
            .strip_prefix(&format!("{name}="))
            .map(str::to_string)
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let auth = headers.get("authorization")?.to_str().ok()?;
    auth.strip_prefix("Bearer ").map(str::to_string)
}

fn parse_job_id(raw: &str) -> Result<JobId, ApiError> {
    raw.parse().map_err(|_| ApiError::not_found("no such job"))
}

fn status_field<'a>(status: &'a str, key: &str) -> Option<&'a str> {
    status.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name == key).then_some(value.trim())
    })
}

fn named_counter(text: &str, key: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name == key {
            value.trim().parse::<u64>().ok()
        } else {
            None
        }
    })
}

fn parse_i32(value: &str) -> Option<i32> {
    value.split_whitespace().next()?.parse().ok()
}

fn parse_u64(value: &str) -> Option<u64> {
    value.split_whitespace().next()?.parse().ok()
}

fn parse_kibibytes(value: &str) -> Option<u64> {
    parse_u64(value)?.checked_mul(1024)
}

fn read_cmdline(path: &FsPath) -> Option<String> {
    let bytes = read_bounded(path, MAX_PROC_TEXT_BYTES).ok()?;
    let parts: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn read_bounded(path: &FsPath, max_bytes: usize) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let max_u64 = u64::try_from(max_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "size limit is too large"))?;
    let mut reader = file.take(max_u64.saturating_add(1));
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    if bytes.len() > max_bytes {
        bytes.truncate(max_bytes);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_selected_proc_status_fields() {
        let status = "Name:\tpython3\nState:\tS (sleeping)\nPPid:\t41\nVmSize:\t2048 kB\nVmRSS:\t512 kB\nThreads:\t7\n";
        assert_eq!(status_field(status, "State"), Some("S (sleeping)"));
        assert_eq!(status_field(status, "PPid").and_then(parse_i32), Some(41));
        assert_eq!(status_field(status, "Threads").and_then(parse_u64), Some(7));
        assert_eq!(
            status_field(status, "VmRSS").and_then(parse_kibibytes),
            Some(512 * 1024)
        );
    }

    #[test]
    fn parses_proc_io_counters() {
        let io = "rchar: 10\nwchar: 20\nread_bytes: 4096\nwrite_bytes: 8192\n";
        assert_eq!(named_counter(io, "read_bytes"), Some(4096));
        assert_eq!(named_counter(io, "write_bytes"), Some(8192));
        assert_eq!(named_counter(io, "cancelled_write_bytes"), None);
    }
}
