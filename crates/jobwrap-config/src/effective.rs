//! Effective configuration with provenance.
//!
//! Precedence (lowest to highest):
//!
//! ```text
//! built-in defaults < user configuration < trusted project configuration
//!     < environment overrides < command-line options
//! ```
//!
//! Each resolved field records a [`ValueSource`] so that diagnostics can report
//! where a value came from.

use std::collections::BTreeMap;

use jobwrap_core::{AccessLevel, ProfileAccess};

use super::{ConfigError, RawConfig};

/// The ordered layers of the configuration precedence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Layer {
    Builtin,
    UserConfig,
    ProjectConfig,
    Environment,
    CommandLine,
}

impl Layer {
    pub fn label(self) -> &'static str {
        match self {
            Layer::Builtin => "built-in defaults",
            Layer::UserConfig => "user configuration",
            Layer::ProjectConfig => "project configuration",
            Layer::Environment => "environment",
            Layer::CommandLine => "command line",
        }
    }
}

/// Where a resolved configuration value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueSource {
    pub layer: Layer,
    /// A human-readable location, e.g. a file path or `env:JOBWRAP_*`.
    pub path: String,
}

/// The names of the access fields in a profile, in stable order.
pub const PROFILE_FIELDS: &[&str] = &[
    "status",
    "output",
    "command",
    "working_directory",
    "send_input",
    "signal_interrupt",
    "signal_terminate",
    "signal_stop",
    "signal_continue",
    "signal_kill",
    "restart",
    "delete",
];

/// A resolved profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveProfile {
    pub name: String,
    pub access: ProfileAccess,
    /// Per-field provenance keyed by field name.
    pub provenance: BTreeMap<&'static str, ValueSource>,
}

/// A fully resolved configuration after all layers have been merged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveConfig {
    pub server: ServerConfig,
    pub daemon: DaemonConfig,
    pub defaults: DefaultsConfig,
    pub authentication: AuthenticationConfig,
    pub profiles: BTreeMap<String, EffectiveProfile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    pub public_base_url: String,
    pub open_browser_on_start: bool,
    pub allow_remote_bind: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    pub auto_start: bool,
    pub idle_shutdown_minutes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefaultsConfig {
    pub profile: String,
    pub allocate_pty: bool,
    pub record_output: bool,
    pub retain_completed_days: u64,
    pub maximum_log_bytes: u64,
    pub show_job_url: bool,
    pub job_name_template: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticationConfig {
    pub trust_local_owner: bool,
    pub browser_session_minutes: u64,
    pub password_attempt_limit: u32,
    pub password_attempt_window_seconds: u64,
}

impl EffectiveConfig {
    /// The built-in defaults, independent of any file.
    pub fn builtin() -> Self {
        let path = "built-in".to_string();
        let mut profiles = BTreeMap::new();
        profiles.insert("standard".to_string(), standard_profile(path.clone()));
        profiles.insert("private".to_string(), private_profile(path));
        Self {
            server: ServerConfig {
                bind: "127.0.0.1".to_string(),
                port: 8765,
                public_base_url: "http://127.0.0.1:8765".to_string(),
                open_browser_on_start: false,
                allow_remote_bind: false,
            },
            daemon: DaemonConfig {
                auto_start: true,
                idle_shutdown_minutes: 0,
            },
            defaults: DefaultsConfig {
                profile: "standard".to_string(),
                allocate_pty: true,
                record_output: true,
                retain_completed_days: 30,
                maximum_log_bytes: 536_870_912,
                show_job_url: true,
                job_name_template: "{executable}-{timestamp}".to_string(),
            },
            authentication: AuthenticationConfig {
                trust_local_owner: true,
                browser_session_minutes: 720,
                password_attempt_limit: 5,
                password_attempt_window_seconds: 60,
            },
            profiles,
        }
    }

    /// Merge a parsed file over the built-in defaults.
    pub fn from_raw(raw: &RawConfig, path: &std::path::Path) -> Result<Self, ConfigError> {
        let mut cfg = Self::builtin();
        let loc = path.display().to_string();

        if let Some(server) = &raw.server {
            if let Some(v) = server.bind.clone() {
                cfg.server.bind = v;
            }
            if let Some(v) = server.port {
                cfg.server.port = v;
            }
            if let Some(v) = &server.public_base_url {
                cfg.server.public_base_url = v.clone();
            }
            if let Some(v) = server.open_browser_on_start {
                cfg.server.open_browser_on_start = v;
            }
            if let Some(v) = server.allow_remote_bind {
                cfg.server.allow_remote_bind = v;
            }
        }
        if let Some(daemon) = &raw.daemon {
            if let Some(v) = daemon.auto_start {
                cfg.daemon.auto_start = v;
            }
            if let Some(v) = daemon.idle_shutdown_minutes {
                cfg.daemon.idle_shutdown_minutes = v;
            }
        }
        if let Some(defaults) = &raw.defaults {
            if let Some(v) = &defaults.profile {
                cfg.defaults.profile = v.clone();
            }
            if let Some(v) = defaults.allocate_pty {
                cfg.defaults.allocate_pty = v;
            }
            if let Some(v) = defaults.record_output {
                cfg.defaults.record_output = v;
            }
            if let Some(v) = defaults.retain_completed_days {
                cfg.defaults.retain_completed_days = v;
            }
            if let Some(v) = defaults.maximum_log_bytes {
                cfg.defaults.maximum_log_bytes = v;
            }
            if let Some(v) = defaults.show_job_url {
                cfg.defaults.show_job_url = v;
            }
            if let Some(v) = &defaults.job_name_template {
                cfg.defaults.job_name_template = v.clone();
            }
        }
        if let Some(auth) = &raw.authentication {
            if let Some(v) = auth.trust_local_owner {
                cfg.authentication.trust_local_owner = v;
            }
            if let Some(v) = auth.browser_session_minutes {
                cfg.authentication.browser_session_minutes = v;
            }
            if let Some(v) = auth.password_attempt_limit {
                cfg.authentication.password_attempt_limit = v;
            }
            if let Some(v) = auth.password_attempt_window_seconds {
                cfg.authentication.password_attempt_window_seconds = v;
            }
        }
        for (name, profile_file) in &raw.profiles {
            let entry = cfg.profiles.entry(name.clone()).or_insert_with(|| {
                // Start from the standard built-in defaults for new profiles.
                standard_profile(format!("built-in:{name}"))
            });
            apply_profile(entry, profile_file, &loc);
        }
        Ok(cfg)
    }

    /// Resolve the profile selected by [`DefaultsConfig::profile`].
    pub fn selected_profile(&self) -> Result<&EffectiveProfile, ConfigError> {
        self.profiles
            .get(&self.defaults.profile)
            .ok_or_else(|| ConfigError::UnknownProfile {
                name: self.defaults.profile.clone(),
            })
    }
}

fn standard_profile(loc: String) -> EffectiveProfile {
    let mut provenance = BTreeMap::new();
    let access = ProfileAccess {
        status: AccessLevel::Public,
        output: AccessLevel::Public,
        command: AccessLevel::Authenticated,
        working_directory: AccessLevel::Authenticated,
        send_input: AccessLevel::Controller,
        signal_interrupt: AccessLevel::Controller,
        signal_terminate: AccessLevel::Controller,
        signal_stop: AccessLevel::Controller,
        signal_continue: AccessLevel::Controller,
        signal_kill: AccessLevel::Owner,
        restart: AccessLevel::Owner,
        delete: AccessLevel::Owner,
    };
    for field in PROFILE_FIELDS {
        provenance.insert(
            *field,
            ValueSource {
                layer: Layer::Builtin,
                path: loc.clone(),
            },
        );
    }
    EffectiveProfile {
        name: "standard".to_string(),
        access,
        provenance,
    }
}

fn private_profile(loc: String) -> EffectiveProfile {
    let mut provenance = BTreeMap::new();
    let access = ProfileAccess {
        status: AccessLevel::Authenticated,
        output: AccessLevel::Authenticated,
        command: AccessLevel::Authenticated,
        working_directory: AccessLevel::Authenticated,
        send_input: AccessLevel::Owner,
        signal_interrupt: AccessLevel::Owner,
        signal_terminate: AccessLevel::Owner,
        signal_stop: AccessLevel::Owner,
        signal_continue: AccessLevel::Owner,
        signal_kill: AccessLevel::Owner,
        restart: AccessLevel::Owner,
        delete: AccessLevel::Owner,
    };
    for field in PROFILE_FIELDS {
        provenance.insert(
            *field,
            ValueSource {
                layer: Layer::Builtin,
                path: loc.clone(),
            },
        );
    }
    EffectiveProfile {
        name: "private".to_string(),
        access,
        provenance,
    }
}

fn apply_profile(profile: &mut EffectiveProfile, file: &super::ProfileFile, loc: &str) {
    let source = ValueSource {
        layer: Layer::UserConfig,
        path: loc.to_string(),
    };
    macro_rules! set {
        ($field:ident) => {
            if let Some(v) = file.$field {
                profile.access.$field = v;
                profile
                    .provenance
                    .insert(stringify!($field), source.clone());
            }
        };
    }
    set!(status);
    set!(output);
    set!(command);
    set!(working_directory);
    set!(send_input);
    set!(signal_interrupt);
    set!(signal_terminate);
    set!(signal_stop);
    set!(signal_continue);
    set!(signal_kill);
    set!(restart);
    set!(delete);
}
