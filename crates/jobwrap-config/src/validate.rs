//! Validation of the effective configuration.

use std::fmt;

use super::EffectiveConfig;

/// A single validation issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationIssue {
    /// Binding to a non-loopback address without TLS is unsafe.
    UnsafeBind {
        bind: String,
        port: u16,
        allow_remote_bind: bool,
    },
    /// A password attempt limit of zero would allow unlimited attempts.
    ZeroAttemptLimit,
    /// A session length of zero disables sessions immediately.
    ZeroSessionMinutes,
    /// The selected profile does not exist.
    UnknownProfile { name: String },
    InvalidLimit {
        field: &'static str,
        value: u64,
        range: &'static str,
    },
}

impl fmt::Display for ValidationIssue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ValidationIssue::UnsafeBind {
                bind,
                port,
                allow_remote_bind,
            } => {
                write!(
                    f,
                    "server binds to {bind}:{port} which is not loopback; \
                     allow_remote_bind = {allow_remote_bind} but no TLS is configured \
                     (use ssh -L or a reverse proxy instead)"
                )
            }
            ValidationIssue::ZeroAttemptLimit => {
                f.write_str("password_attempt_limit = 0 disables rate limiting")
            }
            ValidationIssue::ZeroSessionMinutes => {
                f.write_str("browser_session_minutes = 0 expires sessions immediately")
            }
            ValidationIssue::UnknownProfile { name } => {
                write!(f, "default profile `{name}` is not defined")
            }
            ValidationIssue::InvalidLimit {
                field,
                value,
                range,
            } => {
                write!(f, "{field} = {value} is outside the safe range {range}")
            }
        }
    }
}

/// Return all validation issues found in the configuration.
pub fn validate_effective(cfg: &EffectiveConfig) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let loopback = cfg.server.bind == "localhost"
        || cfg
            .server
            .bind
            .parse::<std::net::IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false);
    if !loopback {
        issues.push(ValidationIssue::UnsafeBind {
            bind: cfg.server.bind.clone(),
            port: cfg.server.port,
            allow_remote_bind: cfg.server.allow_remote_bind,
        });
    }
    if cfg.authentication.password_attempt_limit == 0 {
        issues.push(ValidationIssue::ZeroAttemptLimit);
    }
    if cfg.authentication.browser_session_minutes == 0 {
        issues.push(ValidationIssue::ZeroSessionMinutes);
    }
    if !cfg.profiles.contains_key(&cfg.defaults.profile) {
        issues.push(ValidationIssue::UnknownProfile {
            name: cfg.defaults.profile.clone(),
        });
    }
    if cfg.defaults.maximum_log_bytes == 0
        || cfg.defaults.maximum_log_bytes > 4 * 1024 * 1024 * 1024
    {
        issues.push(ValidationIssue::InvalidLimit {
            field: "defaults.maximum_log_bytes",
            value: cfg.defaults.maximum_log_bytes,
            range: "1..=4294967296",
        });
    }
    if cfg.launch.enabled {
        for (field, value) in [
            (
                "launch.maximum_concurrent_jobs",
                cfg.launch.maximum_concurrent_jobs,
            ),
            (
                "launch.maximum_pending_launches",
                cfg.launch.maximum_pending_launches,
            ),
        ] {
            if value == 0 || value > 100 {
                issues.push(ValidationIssue::InvalidLimit {
                    field,
                    value,
                    range: "1..=100",
                });
            }
        }
    }
    if cfg.help.allow_interpreter_probes || cfg.help.allow_script_probes {
        if cfg.help.probe_timeout_seconds == 0 || cfg.help.probe_timeout_seconds > 30 {
            issues.push(ValidationIssue::InvalidLimit {
                field: "help.probe_timeout_seconds",
                value: cfg.help.probe_timeout_seconds,
                range: "1..=30",
            });
        }
        if cfg.help.probe_output_limit_bytes == 0
            || cfg.help.probe_output_limit_bytes > 8 * 1024 * 1024
        {
            issues.push(ValidationIssue::InvalidLimit {
                field: "help.probe_output_limit_bytes",
                value: cfg.help.probe_output_limit_bytes,
                range: "1..=8388608",
            });
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_safe() {
        let cfg = EffectiveConfig::builtin();
        assert!(validate_effective(&cfg).is_empty());
    }

    #[test]
    fn remote_bind_is_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = "0.0.0.0".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnsafeBind { .. })));
    }

    #[test]
    fn unknown_profile_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.defaults.profile = "nope".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnknownProfile { .. })));
    }
}
