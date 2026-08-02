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
        }
    }
}

/// Return all validation issues found in the configuration.
pub fn validate_effective(cfg: &EffectiveConfig) -> Vec<ValidationIssue> {
    let mut issues = Vec::new();
    if !cfg.server.bind.starts_with("127.")
        && !cfg.server.bind.starts_with("::1")
        && cfg.server.bind != "localhost"
    {
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
