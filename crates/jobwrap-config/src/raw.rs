//! Raw TOML configuration model.
//!
//! Unknown fields are rejected so that typos in authorization settings cannot
//! silently weaken policy.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::{ConfigError, CONFIG_VERSION};

/// The on-disk configuration file structure.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawConfig {
    pub config_version: i32,
    #[serde(default)]
    pub server: Option<ServerFile>,
    #[serde(default)]
    pub daemon: Option<DaemonFile>,
    #[serde(default)]
    pub defaults: Option<DefaultsFile>,
    #[serde(default)]
    pub authentication: Option<AuthenticationFile>,
    #[serde(default)]
    pub launch: Option<LaunchFile>,
    #[serde(default)]
    pub terminal: Option<TerminalFile>,
    #[serde(default)]
    pub help: Option<HelpFile>,
    #[serde(default)]
    pub profiles: BTreeMap<String, ProfileFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerFile {
    #[serde(default)]
    pub bind: Option<String>,
    #[serde(default)]
    pub port: Option<u16>,
    #[serde(default)]
    pub public_base_url: Option<String>,
    #[serde(default)]
    pub open_browser_on_start: Option<bool>,
    #[serde(default)]
    pub allow_remote_bind: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaemonFile {
    #[serde(default)]
    pub auto_start: Option<bool>,
    #[serde(default)]
    pub idle_shutdown_minutes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefaultsFile {
    #[serde(default)]
    pub profile: Option<String>,
    #[serde(default)]
    pub allocate_pty: Option<bool>,
    #[serde(default)]
    pub record_output: Option<bool>,
    #[serde(default)]
    pub retain_completed_days: Option<u64>,
    #[serde(default)]
    pub maximum_log_bytes: Option<u64>,
    #[serde(default)]
    pub show_job_url: Option<bool>,
    #[serde(default)]
    pub job_name_template: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticationFile {
    #[serde(default)]
    pub trust_local_owner: Option<bool>,
    #[serde(default)]
    pub browser_session_minutes: Option<u64>,
    #[serde(default)]
    pub password_attempt_limit: Option<u32>,
    #[serde(default)]
    pub password_attempt_window_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LaunchFile {
    #[serde(default)]
    pub enabled: Option<bool>,
    #[serde(default)]
    pub default_profile: Option<String>,
    #[serde(default)]
    pub default_terminal_mode: Option<String>,
    #[serde(default)]
    pub require_preview: Option<bool>,
    #[serde(default)]
    pub require_idempotency_key: Option<bool>,
    #[serde(default)]
    pub maximum_concurrent_jobs: Option<u64>,
    #[serde(default)]
    pub maximum_pending_launches: Option<u64>,
    #[serde(default)]
    pub preview_lifetime_seconds: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalFile {
    #[serde(default)]
    pub preferred_backend: Option<String>,
    #[serde(default)]
    pub allow_api_backend_selection: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelpFile {
    #[serde(default)]
    pub assume_help_available: Option<bool>,
    #[serde(default)]
    pub prefer_man_pages: Option<bool>,
    #[serde(default)]
    pub allow_interpreter_probes: Option<bool>,
    #[serde(default)]
    pub allow_script_probes: Option<bool>,
    #[serde(default)]
    pub default_probe_argument: Option<String>,
    #[serde(default)]
    pub probe_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub probe_output_limit_bytes: Option<u64>,
    #[serde(default)]
    pub cache_results: Option<bool>,
}

/// A named access profile.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    #[serde(default)]
    pub status: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub output: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub command: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub working_directory: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub send_input: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub signal_interrupt: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub signal_terminate: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub signal_stop: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub signal_continue: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub signal_kill: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub restart: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub delete: Option<jobwrap_core::AccessLevel>,
    #[serde(default)]
    pub launch: Option<jobwrap_core::AccessLevel>,
}

/// Parse raw TOML, rejecting unknown fields and unsupported versions.
pub fn parse(text: &str, path: &std::path::Path) -> Result<RawConfig, ConfigError> {
    let config: RawConfig = toml::from_str(text).map_err(|e| {
        // `toml::de::Error` reports the span with the line number.
        ConfigError::Toml {
            path: path.to_path_buf(),
            message: e.to_string(),
        }
    })?;
    if config.config_version != CONFIG_VERSION {
        return Err(ConfigError::UnsupportedVersion {
            found: config.config_version,
            supported: CONFIG_VERSION,
            path: path.to_path_buf(),
        });
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_example_parses() {
        let text = r#"
config_version = 1

[server]
bind = "127.0.0.1"
port = 8765
public_base_url = "http://127.0.0.1:8765"
open_browser_on_start = false
allow_remote_bind = false

[daemon]
auto_start = true
idle_shutdown_minutes = 0

[defaults]
profile = "standard"
allocate_pty = true
record_output = true
retain_completed_days = 30
maximum_log_bytes = 536870912
show_job_url = true
job_name_template = "{executable}-{timestamp}"

[authentication]
trust_local_owner = true
browser_session_minutes = 720
password_attempt_limit = 5
password_attempt_window_seconds = 60

[profiles.standard]
status = "public"
output = "public"
command = "authenticated"
working_directory = "authenticated"
send_input = "controller"
signal_interrupt = "controller"
signal_terminate = "controller"
signal_stop = "controller"
signal_continue = "controller"
signal_kill = "owner"
restart = "owner"
delete = "owner"

[profiles.private]
status = "authenticated"
output = "authenticated"
command = "authenticated"
working_directory = "authenticated"
send_input = "owner"
signal_interrupt = "owner"
signal_terminate = "owner"
signal_stop = "owner"
signal_continue = "owner"
signal_kill = "owner"
restart = "owner"
delete = "owner"
"#;
        let raw = parse(text, std::path::Path::new("example.toml")).expect("parses");
        assert_eq!(raw.server.as_ref().expect("server").port, Some(8765));
        assert_eq!(raw.profiles.len(), 2);
    }

    #[test]
    fn case_insensitive_access_levels() {
        let text = r#"
config_version = 1
[profiles.standard]
output = "PUBLIC"
"#;
        // AccessLevel is case-sensitive serde; uppercase should be rejected.
        assert!(parse(text, std::path::Path::new("test.toml")).is_err());
    }
}
