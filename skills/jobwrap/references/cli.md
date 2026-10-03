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
jobwrap wrap --name large-upload --profile private -- \
  rclone copy ./dataset remote:dataset --progress
jobwrap wrap --name long-calculation --profile private -- \
  python3 analysis.py --config analysis.yaml
```

Run the wrapper in the desired working directory. It inherits the launch
environment. The launch blocks until the child finishes and returns the child's
exit code. Keep that execution session alive; a tool timeout may terminate the
wrapper or its child.

`jobwrap python3 analysis.py` is shorthand. Use the explicit `wrap ... --`
form to separate wrapper flags from child flags and avoid clashes with reserved
subcommands such as `list`, `show`, or `stop`.

The banner prints `registered job JOB_ID` and its web URL. Save that ID; do not
identify a job by a nonunique name or stale PID. If registration fails, the child
can still be running attached to the terminal without monitoring.

Wrapper options:

| Option | Behavior |
| --- | --- |
| `--name NAME` / `-n NAME` | Human-readable job name |
| `--profile PROFILE` | Select configured access profile |
| `--no-record` | Disable recorded terminal output |
| `--no-web` | Skip daemon registration; no API/web monitoring for this job |
| `--detach` | Not implemented; rejects the request before starting a child |

## Monitor and control

```sh
jobwrap list
jobwrap show JOB_ID
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

`list` and `show` produce human-readable output, not JSON. Prefer HTTP for
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
