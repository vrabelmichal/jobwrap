# Configuration

The user configuration lives at:

```text
~/.config/jobwrap/config.toml
```

`config_version = 1` is required. Unknown fields are rejected rather than
silently ignored, especially authorization settings. `jobwrap config validate`
reports issues; `jobwrap config explain <field>` shows where a resolved value
came from.

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

## Precedence

```text
built-in defaults < user configuration < named profile
  < trusted project configuration < environment < command-line options
```

## XDG paths

```text
$XDG_RUNTIME_DIR/jobwrap/        runtime (sockets, pid, lock)
$XDG_CONFIG_HOME/jobwrap/        configuration
$XDG_STATE_HOME/jobwrap/         database and terminal logs
$XDG_DATA_HOME/jobwrap/          web assets
```
