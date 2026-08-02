//! XDG base directory handling.
//!
//! Runtime files live under `$XDG_RUNTIME_DIR/jobwrap/`, persistent state under
//! `$XDG_STATE_HOME/jobwrap/`, configuration under `$XDG_CONFIG_HOME/jobwrap/`,
//! and web assets under `$XDG_DATA_HOME/jobwrap/`.

use std::path::PathBuf;

use super::ConfigError;

fn env_or(var: &str, fallback: impl FnOnce() -> PathBuf) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(fallback)
}

fn home() -> Result<PathBuf, ConfigError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| ConfigError::Xdg("HOME is not set".to_string()))
}

/// Persistent configuration and state paths.
#[derive(Debug, Clone)]
pub struct ConfigPaths {
    pub config_file: PathBuf,
    pub state_dir: PathBuf,
    pub db_file: PathBuf,
    pub logs_dir: PathBuf,
    pub data_dir: PathBuf,
}

impl ConfigPaths {
    pub fn discover() -> Result<Self, ConfigError> {
        let home = home()?;

        let config_home = env_or("XDG_CONFIG_HOME", || home.join(".config"));
        let config_dir = config_home.join("jobwrap");
        let config_file = config_dir.join("config.toml");

        let state_home = env_or("XDG_STATE_HOME", || home.join(".local/state"));
        let state_dir = state_home.join("jobwrap");
        let db_file = state_dir.join("jobwrap.db");
        let logs_dir = state_dir.join("logs");

        let data_home = env_or("XDG_DATA_HOME", || home.join(".local/share"));
        let data_dir = data_home.join("jobwrap");

        Ok(Self {
            config_file,
            state_dir,
            db_file,
            logs_dir,
            data_dir,
        })
    }
}

/// Ephemeral runtime paths (sockets, pid, lock).
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub dir: PathBuf,
    pub daemon_socket: PathBuf,
    pub daemon_pid: PathBuf,
    pub daemon_lock: PathBuf,
    pub http_ready_file: PathBuf,
    /// Persistent append-only output logs.
    pub logs_dir: PathBuf,
}

impl RuntimePaths {
    pub fn discover() -> Result<Self, ConfigError> {
        let home = home()?;
        let runtime_home = env_or("XDG_RUNTIME_DIR", || home.join(".runtime"));
        let dir = runtime_home.join("jobwrap");
        let state_home = env_or("XDG_STATE_HOME", || home.join(".local/state"));
        Ok(Self {
            dir: dir.clone(),
            daemon_socket: dir.join("daemon.sock"),
            daemon_pid: dir.join("daemon.pid"),
            daemon_lock: dir.join("daemon.lock"),
            http_ready_file: dir.join("http.port"),
            logs_dir: state_home.join("jobwrap").join("logs"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_defaults_when_env_unset() {
        // These tests must not depend on the ambient environment.
        let saved = [
            "HOME",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_DATA_HOME",
            "XDG_RUNTIME_DIR",
        ];
        let mut backups = Vec::new();
        for v in saved {
            backups.push((v, std::env::var_os(v)));
            std::env::remove_var(v);
        }
        std::env::set_var("HOME", "/home/testuser");
        let paths = ConfigPaths::discover().expect("discover");
        assert_eq!(
            paths.config_file,
            PathBuf::from("/home/testuser/.config/jobwrap/config.toml")
        );
        assert_eq!(
            paths.db_file,
            PathBuf::from("/home/testuser/.local/state/jobwrap/jobwrap.db")
        );
        for (var, old) in backups {
            match old {
                Some(val) => std::env::set_var(var, val),
                None => std::env::remove_var(var),
            }
        }
    }
}
