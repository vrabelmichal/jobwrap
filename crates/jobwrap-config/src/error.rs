//! Configuration errors.

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("could not read configuration file {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid configuration in {path}: {message}")]
    Toml { path: PathBuf, message: String },
    #[error("unsupported config_version {found}; this binary supports version {supported}")]
    UnsupportedVersion {
        found: i32,
        supported: i32,
        path: PathBuf,
    },
    #[error("unknown profile `{name}`")]
    UnknownProfile { name: String },
    #[error("environment variable `{name}` is invalid: {message}")]
    Environment { name: String, message: String },
    #[error("invalid XDG path: {0}")]
    Xdg(String),
}
