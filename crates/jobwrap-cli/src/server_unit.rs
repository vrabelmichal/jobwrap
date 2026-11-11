//! Generate a reviewable unit instead of modifying the user's service manager.
use anyhow::{bail, Context};
use std::ffi::OsString;

fn quote(value: &str, exec: bool) -> String {
    let mut escaped = String::from("\"");
    for c in value.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            '%' => escaped.push_str("%%"),
            '$' if exec => escaped.push_str("$$"),
            c if c.is_control() => escaped.push_str(&format!("\\x{:02x}", c as u32)),
            c => escaped.push(c),
        }
    }
    escaped.push('"');
    escaped
}

pub(crate) fn render(name: &str, profile: &str, command: &[OsString]) -> anyhow::Result<String> {
    jobwrap_core::JobName::new(name)?;
    if command.is_empty() {
        bail!("server command is required");
    }
    let executable = std::env::current_exe()?;
    let cwd = std::env::current_dir()?;
    let cwd = cwd.to_str().context("UTF-8 working directory required")?;
    // WorkingDirectory is a raw path directive, unlike ExecStart and
    // Environment: quoting adds literal quotation marks to the path.
    if cwd.chars().any(|c| c.is_control() || c == '\\') || cwd.trim_end() != cwd {
        bail!("persistent service working directories cannot contain controls, backslashes or trailing whitespace");
    }
    let mut argv = vec![
        executable.into_os_string(),
        "run".into(),
        "--name".into(),
        name.into(),
        "--profile".into(),
        profile.into(),
        "--".into(),
    ];
    argv.extend_from_slice(command);
    let argv: Result<Vec<_>, _> = argv
        .iter()
        .map(|s| {
            s.to_str()
                .context("persistent service definitions require UTF-8 arguments")
                .map(|s| quote(s, true))
        })
        .collect();
    let mut unit = format!("# Save as jobwrap-server-{name}.service in your systemd user unit directory.\n# Review, then enable with: systemctl --user enable --now jobwrap-server-{name}.service\n# Foreground run belongs to this service; do not add --detach.\n[Unit]\nDescription=jobwrap persistent server {name}\n\n[Service]\nType=exec\nWorkingDirectory={}\nExecStart={}\nRestart=on-failure\nRestartSec=5\nKillMode=control-group\nTimeoutStopSec=30\n", cwd.replace('%', "%%"), argv?.join(" "));
    // Deliberately omit credentials and the rest of the caller environment.
    // These paths preserve Python activation and isolated control-plane setup.
    for key in [
        "PATH",
        "VIRTUAL_ENV",
        "CONDA_PREFIX",
        "UV_PROJECT_ENVIRONMENT",
        "PYTHONPATH",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_DATA_HOME",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Some(value) = std::env::var_os(key) {
            let value = value
                .to_str()
                .context("persistent service environment paths require UTF-8")?;
            unit.push_str(&format!(
                "Environment={}\n",
                quote(&format!("{key}={value}"), false)
            ));
        }
    }
    unit.push_str("\n[Install]\nWantedBy=default.target\n");
    Ok(unit)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unit_escapes_systemd_expansions_without_shell_interpretation() {
        assert_eq!(quote("$HOME/%i\n\"", true), "\"$$HOME/%%i\\n\\\"\"");
        assert_eq!(quote("$HOME/%i", false), "\"$HOME/%%i\"");
        let unit = render(
            "review",
            "private",
            &["python".into(), "-m".into(), "http.server".into()],
        )
        .unwrap();
        assert!(unit.contains("Restart=on-failure"));
        assert!(!unit
            .lines()
            .find(|line| line.starts_with("ExecStart="))
            .unwrap()
            .contains("--detach"));
        assert!(render("invalid/name", "private", &["true".into()]).is_err());
    }
}
