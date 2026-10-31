//! Questions to the systemd user manager about detached jobs' services.
//!
//! A detached job runs in its own transient user service. When the daemon
//! loses the worker connection (for example after a daemon restart), the
//! service manager is the only authority left on whether the workload still
//! runs. Only read-only `systemctl --user show` queries are made here.

use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use chrono::{NaiveDateTime, TimeZone, Utc};
use jobwrap_protocol::ServiceUnitStatus;

const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const PROPERTIES: &str = "LoadState,ActiveState,SubState,Result,MainPID,ActiveEnterTimestamp,\
                          MemoryCurrent,CPUUsageNSec,TasksCurrent";

/// Only units jobwrap itself names are ever queried.
fn is_jobwrap_unit(unit: &str) -> bool {
    unit.strip_prefix("jobwrap-job-")
        .and_then(|rest| rest.strip_suffix(".service"))
        .is_some_and(|id| id.parse::<jobwrap_core::JobId>().is_ok())
}

fn unavailable(unit: &str, error: impl Into<String>) -> ServiceUnitStatus {
    ServiceUnitStatus {
        unit: unit.to_string(),
        available: false,
        error: Some(error.into()),
        ..ServiceUnitStatus::default()
    }
}

/// Ask the user manager about `unit`. Blocks for at most a few seconds.
pub fn query(unit: &str) -> ServiceUnitStatus {
    if !is_jobwrap_unit(unit) {
        return unavailable(unit, "not a jobwrap job service name");
    }
    let child = Command::new("systemctl")
        .args(["--user", "show", "--no-pager"])
        .arg(format!("--property={PROPERTIES}"))
        .arg("--")
        .arg(unit)
        // Timestamps are printed in local time; UTC makes them parseable.
        .env("TZ", "UTC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return unavailable(unit, format!("could not run systemctl: {error}")),
    };
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return unavailable(unit, "systemctl timed out");
            }
            Err(error) => return unavailable(unit, format!("waiting for systemctl: {error}")),
        }
    };
    // The output is a few hundred bytes, far below the pipe buffer size, so
    // reading after exit cannot deadlock.
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    if !status.success() {
        let message = stderr.trim();
        return unavailable(
            unit,
            if message.is_empty() {
                format!("systemctl failed with {status}")
            } else {
                message.chars().take(300).collect()
            },
        );
    }
    parse_show(unit, &stdout)
}

/// Parse `systemctl show` `Key=Value` output.
pub fn parse_show(unit: &str, text: &str) -> ServiceUnitStatus {
    let mut status = ServiceUnitStatus {
        unit: unit.to_string(),
        available: true,
        ..ServiceUnitStatus::default()
    };
    let text_value = |value: &str| (!value.is_empty()).then(|| value.to_string());
    // Unset counters print `[not set]` or u64::MAX.
    let counter = |value: &str| value.parse::<u64>().ok().filter(|v| *v != u64::MAX);
    for line in text.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key {
            "LoadState" => status.load_state = text_value(value),
            "ActiveState" => status.active_state = text_value(value),
            "SubState" => status.sub_state = text_value(value),
            "Result" => status.result = text_value(value),
            "MainPID" => status.main_pid = value.parse().ok().filter(|pid| *pid > 0),
            "ActiveEnterTimestamp" => status.active_since = parse_timestamp(value),
            "MemoryCurrent" => status.memory_bytes = counter(value),
            "CPUUsageNSec" => status.cpu_usage_nanoseconds = counter(value),
            "TasksCurrent" => status.tasks = counter(value),
            _ => {}
        }
    }
    status
}

/// Parse `Wed 2026-10-21 16:36:04 UTC` as printed with `TZ=UTC`.
fn parse_timestamp(value: &str) -> Option<chrono::DateTime<Utc>> {
    let rest = value.split_once(' ')?.1;
    let rest = rest.strip_suffix(" UTC")?;
    let naive = NaiveDateTime::parse_from_str(rest, "%Y-%m-%d %H:%M:%S").ok()?;
    Some(Utc.from_utc_datetime(&naive))
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNIT: &str = "jobwrap-job-01M69SPGF9T269Y2DQZ2DP1MWW.service";

    #[test]
    fn parses_an_active_service() {
        let status = parse_show(
            UNIT,
            "MainPID=4242\nResult=success\nMemoryCurrent=1048576\nCPUUsageNSec=250000000\n\
             TasksCurrent=3\nLoadState=loaded\nActiveState=active\nSubState=running\n\
             ActiveEnterTimestamp=Wed 2026-10-21 16:36:04 UTC\n",
        );
        assert!(status.is_running());
        assert!(!status.has_ended());
        assert_eq!(status.main_pid, Some(4242));
        assert_eq!(status.memory_bytes, Some(1_048_576));
        assert_eq!(status.tasks, Some(3));
        assert_eq!(
            status.active_since.map(|t| t.to_rfc3339()),
            Some("2026-10-21T16:36:04+00:00".to_string())
        );
        assert_eq!(status.describe(), "active (running)");
    }

    #[test]
    fn a_collected_unit_has_ended() {
        let status = parse_show(
            UNIT,
            "MainPID=0\nResult=success\nMemoryCurrent=[not set]\nCPUUsageNSec=18446744073709551615\n\
             TasksCurrent=[not set]\nLoadState=not-found\nActiveState=inactive\nSubState=dead\n\
             ActiveEnterTimestamp=\n",
        );
        assert!(status.has_ended());
        assert!(!status.is_running());
        assert_eq!(status.main_pid, None);
        assert_eq!(status.cpu_usage_nanoseconds, None);
        assert_eq!(status.active_since, None);
    }

    #[test]
    fn an_unreachable_manager_proves_nothing() {
        let status = unavailable(UNIT, "Failed to connect to bus");
        assert!(!status.has_ended());
        assert!(!status.is_running());
        assert!(status.describe().contains("Failed to connect"));
    }

    #[test]
    fn only_jobwrap_units_are_queried() {
        assert!(is_jobwrap_unit(UNIT));
        for unit in [
            "jobwrapd.service",
            "jobwrap-job-nope.service",
            "jobwrap-job-01M69SPGF9T269Y2DQZ2DP1MWW.socket",
            "--user",
        ] {
            assert!(!is_jobwrap_unit(unit), "{unit}");
            assert!(!query(unit).available, "{unit}");
        }
    }
}
