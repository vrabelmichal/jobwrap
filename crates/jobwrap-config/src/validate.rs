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
    /// A selected profile does not exist.
    UnknownProfile { name: String },
    /// A safety-critical switch was disabled.
    RequiredSafetySetting { field: &'static str },
    /// A string setting does not name a supported implementation.
    InvalidChoice {
        field: &'static str,
        value: String,
        allowed: &'static str,
    },
    InvalidLimit {
        field: &'static str,
        value: u64,
        range: &'static str,
    },
    /// A configured bind value is not a supported target.
    InvalidBind { value: String },
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
                write!(f, "configured profile `{name}` is not defined")
            }
            ValidationIssue::RequiredSafetySetting { field } => {
                write!(f, "{field} must be true")
            }
            ValidationIssue::InvalidChoice {
                field,
                value,
                allowed,
            } => {
                write!(f, "{field} = `{value}` is unsupported; expected {allowed}")
            }
            ValidationIssue::InvalidLimit {
                field,
                value,
                range,
            } => {
                write!(f, "{field} = {value} is outside the safe range {range}")
            }
            ValidationIssue::InvalidBind { value } => {
                write!(
                    f,
                    "server.bind = `{value}` is not a supported bind target \
                     (use an IP address, \"loopback\", \"tailscale\", or a \
                     comma-separated list of those)"
                )
            }
        }
    }
}

/// Whether a single bind target is as private as loopback.
fn is_loopback_target(target: &str) -> bool {
    matches!(target, "loopback" | "tailscale" | "localhost")
        || target
            .parse::<std::net::IpAddr>()
            .map(|address| address.is_loopback())
            .unwrap_or(false)
}

/// Whether a single bind target is recognized at all.
fn is_known_target(target: &str) -> bool {
    matches!(
        target,
        "loopback" | "tailscale" | "localhost" | "0.0.0.0" | "::" | "127.0.0.1" | "::1"
    ) || target.parse::<std::net::IpAddr>().is_ok()
}

/// Return all validation issues found in the configuration.
pub fn validate_effective(cfg: &EffectiveConfig) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    let targets: Vec<&str> = cfg
        .server
        .bind
        .split(',')
        .map(str::trim)
        .filter(|target| !target.is_empty())
        .collect();
    // "tailscale" names the authenticated tailscale0 VPN interface, so it is
    // as private as loopback; every other non-loopback bind target is
    // refused (see docs/security-model.md).
    let loopback = !targets.is_empty() && targets.iter().all(|target| is_loopback_target(target));
    if !loopback {
        issues.push(ValidationIssue::UnsafeBind {
            bind: cfg.server.bind.clone(),
            port: cfg.server.port,
            allow_remote_bind: cfg.server.allow_remote_bind,
        });
    }
    if targets.is_empty() || !targets.iter().all(|target| is_known_target(target)) {
        issues.push(ValidationIssue::InvalidBind {
            value: cfg.server.bind.clone(),
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
        if !cfg.profiles.contains_key(&cfg.launch.default_profile) {
            issues.push(ValidationIssue::UnknownProfile {
                name: cfg.launch.default_profile.clone(),
            });
        }
        if !cfg.launch.require_idempotency_key {
            issues.push(ValidationIssue::RequiredSafetySetting {
                field: "launch.require_idempotency_key",
            });
        }
        if !matches!(
            cfg.launch.default_terminal_mode.as_str(),
            "new-terminal" | "new_terminal"
        ) {
            issues.push(ValidationIssue::InvalidChoice {
                field: "launch.default_terminal_mode",
                value: cfg.launch.default_terminal_mode.clone(),
                allowed: "new-terminal",
            });
        }
        if !matches!(
            cfg.terminal.preferred_backend.as_str(),
            "gnome-terminal" | "xterm"
        ) {
            issues.push(ValidationIssue::InvalidChoice {
                field: "terminal.preferred_backend",
                value: cfg.terminal.preferred_backend.clone(),
                allowed: "gnome-terminal or xterm",
            });
        }
        for (field, value) in [
            (
                "launch.maximum_concurrent_jobs",
                cfg.launch.maximum_concurrent_jobs,
            ),
            (
                "launch.maximum_pending_launches",
                cfg.launch.maximum_pending_launches,
            ),
            (
                "launch.preview_lifetime_seconds",
                cfg.launch.preview_lifetime_seconds,
            ),
        ] {
            let maximum = if field == "launch.preview_lifetime_seconds" {
                600
            } else {
                100
            };
            if value == 0 || value > maximum {
                issues.push(ValidationIssue::InvalidLimit {
                    field,
                    value,
                    range: if maximum == 600 { "1..=600" } else { "1..=100" },
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
    fn tailscale_bind_is_accepted() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = "tailscale".to_string();
        assert!(validate_effective(&cfg).is_empty());
    }

    #[test]
    fn loopback_and_tailscale_list_is_accepted() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = "loopback,tailscale".to_string();
        assert!(validate_effective(&cfg).is_empty());
    }

    #[test]
    fn unsafe_target_in_list_is_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = "loopback,0.0.0.0".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnsafeBind { .. })));
    }

    #[test]
    fn unknown_target_in_list_is_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = "loopback,nope".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::InvalidBind { .. })));
    }

    #[test]
    fn empty_bind_is_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = ",,".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::InvalidBind { .. })));
    }

    #[test]
    fn unknown_bind_target_is_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.server.bind = "not-a-bind".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::InvalidBind { .. })));
    }

    #[test]
    fn unknown_profile_flagged() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.defaults.profile = "nope".to_string();
        assert!(validate_effective(&cfg)
            .iter()
            .any(|i| matches!(i, ValidationIssue::UnknownProfile { .. })));
    }

    #[test]
    fn launch_lifetime_is_bounded_when_launch_is_enabled() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.launch.enabled = true;
        cfg.launch.preview_lifetime_seconds = 601;
        assert!(validate_effective(&cfg).iter().any(|issue| matches!(
            issue,
            ValidationIssue::InvalidLimit {
                field: "launch.preview_lifetime_seconds",
                ..
            }
        )));
    }

    #[test]
    fn launch_rejects_unsupported_or_unsafe_defaults() {
        let mut cfg = EffectiveConfig::builtin();
        cfg.launch.enabled = true;
        cfg.launch.require_idempotency_key = false;
        cfg.launch.default_terminal_mode = "managed".into();
        let issues = validate_effective(&cfg);
        assert!(issues
            .iter()
            .any(|issue| matches!(issue, ValidationIssue::RequiredSafetySetting { .. })));
        assert!(issues
            .iter()
            .any(|issue| matches!(issue, ValidationIssue::InvalidChoice { .. })));
    }
}
