//! jobwrap-config: TOML configuration models, defaults, profile resolution,
//! precedence, validation, and provenance reporting.
#![forbid(unsafe_code)]

mod effective;
mod error;
mod paths;
mod raw;
mod validate;

pub use effective::{
    EffectiveConfig, EffectiveProfile, HelpConfig, LaunchConfig, Layer, TerminalConfig,
    ValueSource, PROFILE_FIELDS,
};
pub use error::ConfigError;
pub use paths::{ConfigPaths, RuntimePaths};
pub use raw::{AuthenticationFile, DaemonFile, DefaultsFile, ProfileFile, RawConfig, ServerFile};
pub use validate::{validate_effective, ValidationIssue};

/// The configuration schema version understood by this binary.
pub const CONFIG_VERSION: i32 = 1;

/// Load and validate a configuration file from disk.
pub fn load_file(path: &std::path::Path) -> Result<EffectiveConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let raw = raw::parse(&text, path)?;
    let effective = effective::EffectiveConfig::from_raw(&raw, path)?;
    let issues = validate::validate_effective(&effective);
    for issue in &issues {
        tracing::warn!(path = %path.display(), issue = %issue, "configuration validation");
    }
    Ok(effective)
}

/// Load the user configuration from the default location.
pub fn load_user_config() -> Result<EffectiveConfig, ConfigError> {
    let paths = ConfigPaths::discover()?;
    load_file(&paths.config_file)
}

/// The built-in defaults, used as the base layer of every configuration.
pub fn builtin_defaults() -> EffectiveConfig {
    EffectiveConfig::builtin()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<EffectiveConfig, ConfigError> {
        let raw = raw::parse(text, std::path::Path::new("test.toml"))?;
        EffectiveConfig::from_raw(&raw, std::path::Path::new("test.toml"))
    }

    #[test]
    fn empty_config_uses_builtin_defaults() {
        let cfg = parse("config_version = 1\n").expect("parses");
        assert_eq!(cfg.server.port, 8765);
        assert_eq!(cfg.server.bind, "127.0.0.1");
        assert_eq!(cfg.defaults.profile, "standard");
        assert_eq!(cfg.authentication.browser_session_minutes, 720);
    }

    #[test]
    fn partial_config_merges_over_defaults() {
        let text = r#"
config_version = 1
[server]
port = 9000
"#;
        let cfg = parse(text).expect("parses");
        assert_eq!(cfg.server.port, 9000);
        assert_eq!(cfg.server.bind, "127.0.0.1");
    }

    #[test]
    fn unknown_top_level_field_rejected() {
        let text = "config_version = 1\nunknown_field = 1\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn unknown_profile_field_rejected() {
        let text = r#"
config_version = 1
[profiles.standard]
signal_interupt = "controller"
"#;
        assert!(parse(text).is_err());
    }

    #[test]
    fn unknown_version_rejected() {
        let text = "config_version = 2\n";
        assert!(matches!(
            parse(text),
            Err(ConfigError::UnsupportedVersion { .. })
        ));
    }

    #[test]
    fn missing_version_rejected() {
        let text = "[server]\nport = 1\n";
        assert!(parse(text).is_err());
    }

    #[test]
    fn profile_access_resolution() {
        let text = r#"
config_version = 1
[profiles.standard]
signal_kill = "disabled"
"#;
        let cfg = parse(text).expect("parses");
        let profile = cfg.profiles.get("standard").expect("standard profile");
        assert_eq!(
            profile.access.signal_kill,
            jobwrap_core::AccessLevel::Disabled
        );
        assert_eq!(profile.access.output, jobwrap_core::AccessLevel::Public);
    }

    #[test]
    fn provenance_tracks_source() {
        let text = r#"
config_version = 1
[profiles.standard]
signal_kill = "disabled"
"#;
        let cfg = parse(text).expect("parses");
        let profile = cfg.profiles.get("standard").expect("standard profile");
        let source = profile
            .provenance
            .get("signal_kill")
            .expect("provenance for signal_kill");
        assert_eq!(source.layer, Layer::UserConfig);
        assert!(source.path.contains("test.toml"));
    }
}
