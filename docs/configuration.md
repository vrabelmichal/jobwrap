# Configuration

The user configuration lives at:

```text
~/.config/jobwrap/config.toml
```

`config_version = 1` is required. Unknown fields are rejected rather than
silently ignored, especially authorization settings. `jobwrap config validate`
reports issues and the daemon refuses unsafe values. `jobwrap config explain
<field>` shows where a profile access value came from. A missing file uses
safe built-in defaults; a malformed existing file is never silently ignored.

## Reference

```toml
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

[launch]
enabled = false
default_profile = "standard"
default_terminal_mode = "new-terminal"
require_preview = true
require_idempotency_key = true
maximum_concurrent_jobs = 20
maximum_pending_launches = 20
preview_lifetime_seconds = 300

[terminal]
preferred_backend = "gnome-terminal"
allow_api_backend_selection = false

[help]
assume_help_available = false
prefer_man_pages = true
allow_interpreter_probes = false
allow_script_probes = false
default_probe_argument = "--help"
probe_timeout_seconds = 3
probe_output_limit_bytes = 1048576
cache_results = true

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
# ... all owner for the private profile
```

## Access levels

Each access field accepts one of:

```text
public         anyone
authenticated  any session or token
controller     logged-in browser session, or token with a matching grant
owner          the local Unix user who launched the job
disabled       nobody
```

`public output` is convenient but may itself contain secrets. Use the `private`
profile for sensitive work.

## Resolution

```text
built-in defaults < user configuration
```

There is currently no project-configuration or environment-override layer.
The wrapper's `--profile` option selects a named profile after loading the
user configuration.

## Safety limits

The HTTP listener must be loopback. Use SSH forwarding or a local reverse
proxy for remote access. Logs stop recording at 512 MiB per job by default and
at 4 GiB total. Probe execution is opt-in and is only environment-limited, not
an operating-system sandbox; do not enable target-executing probes for
untrusted clients.

When launch is enabled, idempotency keys remain mandatory and only the
`new-terminal` mode with the `gnome-terminal` or `xterm` backend is supported.
Managed and existing-terminal launch modes fail closed. The web interface
offers a "new job" page at `/jobs/new` that uses the same launch API; when
creation is disabled or fails closed it explains exactly which settings are
responsible instead of showing a form. The
`retain_completed_days` setting is reserved for future retention support;
completed records are currently removed only with an explicit delete command.

## XDG paths

```text
$XDG_RUNTIME_DIR/jobwrap/        runtime (sockets, pid, lock)
$XDG_CONFIG_HOME/jobwrap/        configuration
$XDG_STATE_HOME/jobwrap/         database and terminal logs
$XDG_DATA_HOME/jobwrap/          web assets
```
