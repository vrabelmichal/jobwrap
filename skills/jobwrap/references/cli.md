# Local CLI

## Start and identify a workload

jobwrap is a Linux program. Both `jobwrap` and its companion `jobwrapd` must be
available for monitored execution. A configured daemon normally starts
automatically when needed; an explicit start prints its web URL.

```sh
jobwrap --version
jobwrap daemon status
jobwrap daemon start

# Illustrative commands: substitute the user's actual executable and arguments.
jobwrap run --detach --name large-upload --profile private -- \
  rclone copy ./dataset remote:dataset --progress
jobwrap run --name long-calculation --profile private -- \
  python3 analysis.py --config analysis.yaml
```

Run the wrapper in the desired working directory. It inherits the launch
environment. Foreground `run` blocks until the child finishes and returns its
exit code. Keep that session alive; a tool timeout may terminate the wrapper
or child. Use `--detach` for jobs that must survive the launching agent.

Version 0.2 requires the explicit `run` subcommand. Bare `jobwrap python3 ...`,
`jobwrap -- COMMAND`, and `jobwrap wrap ...` are rejected before execution.
Place run options before `--` and the executable after it. For example,
`jobwrap run -- status ARG` runs an executable named `status`, whereas
`jobwrap status JOB_ID` is a read-only alias for `show`.

The banner prints `registered job JOB_ID` (or `registered detached job JOB_ID`)
and its web URL. Save that ID; do not identify a job by a nonunique name or stale
PID. Foreground registration failure can leave the child running without
monitoring; detached registration failure terminates and reaps its child.

Wrapper options:

| Option | Behavior |
| --- | --- |
| `--name NAME` / `-n NAME` | Human-readable job name |
| `--profile PROFILE` | Select configured access profile |
| `--no-record` | Disable recorded terminal output |
| `--no-web` | Skip daemon registration; no API/web monitoring for this job |
| `--detach` / `-d` | Run in an independent systemd user service and return after registration |

## Detached servers and tasks

```sh
jobwrap run -d --name report-server --profile private -- \
  python3 -m http.server 18881 --bind 127.0.0.1 --directory ./report
```

Detached mode requires `systemd-run` and a working systemd user manager. The
local owner CLI sends its context to the daemon, which starts a separate user
service outside the agent's process tree and cgroup. It does not require
`[launch] enabled = true` or open the HTTP launch API. The worker preserves
native argv, cwd, and environment bytes; environment values travel only over
the private socket and are not placed in unit arguments or job metadata.

The CLI returns zero after registration. It does not wait for completion or
guarantee that a server is ready. Check `show`/`status`, output, and workload
readiness. On an ambiguous startup timeout, inspect the reported job ID and unit
before retrying. `--no-record` works; `--no-web` conflicts with `--detach`.

Save the printed `jobwrap-job-JOB_ID.service` name. Services survive daemon
shutdown, but the current protocol cannot reconnect/adopt wrappers: their
records become `disconnected`, and daemon recording/control stop. Inspect with
`systemctl --user status jobwrap-job-JOB_ID.service` or
`journalctl --user -u jobwrap-job-JOB_ID.service`; when termination is authorized,
use `systemctl --user stop jobwrap-job-JOB_ID.service` for a disconnected job.
User-manager shutdown, logout policies, reboot, or explicit stops can still
terminate services. No login-lingering, boot persistence, or automatic restart
is configured. Keep files needed by the workload available independently.

## Monitor and control

```sh
jobwrap list
jobwrap show JOB_ID
jobwrap status JOB_ID               # read-only alias for show
jobwrap logs JOB_ID
jobwrap logs JOB_ID --offset 1048576
jobwrap open JOB_ID

# Use only when the user's task authorizes the control action.
jobwrap signal JOB_ID int
jobwrap stop JOB_ID                 # SIGTERM, not suspension
jobwrap signal JOB_ID stop          # SIGSTOP, suspension
jobwrap signal JOB_ID cont          # SIGCONT, resume
jobwrap signal JOB_ID kill          # SIGKILL, forced termination
```

`list`, `show`, and `status` produce human-readable output, not JSON. Detailed
status includes the final exit code or termination signal. Prefer HTTP for
machine-readable monitoring. `logs` fetches one bounded slice (up to 1 MiB);
`--offset` is a byte offset. It is not a follow command. Output contains terminal
bytes and may include ANSI escapes, carriage returns, and non-UTF-8 data.
`attach` and `logs -f` are not implemented; open the web interface for live output
or use HTTP/WebSocket monitoring.

Ctrl+C in the launch terminal interrupts the child's process group. Ctrl+Z
suspends the child and wrapper; shell `fg` resumes them. Piped stdin reaching EOF
does not deliver EOF to the child's PTY, so avoid workflows that depend on that.

## Daemon and configuration

```sh
jobwrap daemon start --foreground
jobwrap config path
jobwrap config validate
jobwrap config effective
jobwrap config explain output
```

Configuration normally lives at `~/.config/jobwrap/config.toml` (respecting
`XDG_CONFIG_HOME`). Missing configuration uses defaults; unknown fields and
invalid settings are rejected. `config init` writes a default configuration;
use it only when configuration creation is wanted. There is no project config
or environment override layer for settings.
`config explain` takes an access-field name such as `output` and explains that
field in the configured default profile.

The default web address is `http://127.0.0.1:8765`; use the printed URL and
effective configuration rather than assuming it. Loopback is the default bind;
explicit `tailscale` binding is also supported. Arbitrary non-loopback binds
are refused even with `allow_remote_bind = true`.

`jobwrap daemon restart` applies configuration changes and `daemon stop` stops
the daemon. These affect monitoring for other jobs too; they are not commands
for cancelling one workload. Completed records are currently removed explicitly;
`retain_completed_days` does not implement automatic retention cleanup.

## Authentication and scoped tokens

The local CLI uses a private Unix socket with same-UID owner authority, without
a browser password. HTTP clients do not inherit that authority just by running
on the same machine.

For authorized token setup, create a read-only token bound to the desired job:

```sh
jobwrap token create --name calculation-monitor --job-id JOB_ID \
  --scope jobs:list \
  --scope job:JOB_ID:status \
  --scope job:JOB_ID:output
jobwrap token list
jobwrap token revoke TOKEN_ID
```

Replace every `JOB_ID` placeholder, including those inside scope strings. The
secret token is printed only once; store it securely. `--expires-at` accepts
RFC3339. For multiple jobs, quoted scopes such as `'job:*:status'` and
`'job:*:output'` cover the owner's jobs. Add control scopes only when needed:
`job:JOB_ID:input`, `job:JOB_ID:signal:int`, `:signal:term`, `:signal:stop`, or
`:signal:cont`. Scope suffixes and HTTP signal names differ; see the API reference.
Token grants remain subject to the job's access policy; an owner-only operation
cannot be made available merely by adding a token scope.

`jobwrap auth status` reports browser authentication setup.
`jobwrap auth set-password` uses an interactive non-echoing terminal prompt.
Password removal and token revocation change access and require task authority.
