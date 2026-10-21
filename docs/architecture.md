# Architecture

jobwrap wraps a locally launched Linux process in a pseudo-terminal and exposes
secure monitoring and narrowly authorized control through a per-user daemon.

## Components

```text
GNOME Terminal / shell
   |
   +-- jobwrap run -- COMMAND ARGS   (/usr/bin/jobwrap)
        |
        +-- PTY relay  <---> child process group (command + descendants)
        |
        +-- private Unix socket
             |
             v
        jobwrapd                     (/usr/libexec/jobwrap/jobwrapd)
        |-- job registry
        |-- authorization engine
        |-- event/log store (SQLite + append-only terminal logs)
        |-- Unix-socket API
        |-- HTTP API (Axum)
        |-- WebSocket output/input
        `-- browser interface
```

The system is two cooperating executables:

* **`jobwrap`** — the command users type. It parses arguments, allocates a PTY,
  forks the command into its own session and process group, registers the job
  with the daemon, and relays terminal traffic. In detached mode a new copy
  runs as a separate systemd user service rather than as an agent descendant.
* **`jobwrapd`** — a per-user daemon providing the web interface, HTTP API,
  authentication, job registry, log storage, and access control. Experimental
  HTTP launch/probe features are disabled by default. The separate local-owner
  detached-launch path starts a fixed worker through the user service manager.
  The opt-in HTTP launch mode starts `jobwrap attach-launch` in a terminal.

## Process model

When invoked, `jobwrap`:

1. requires the `run` subcommand and parses its options before the executable;
2. loads the user configuration;
3. determines the effective profile;
4. allocates a new pseudo-terminal;
5. forks and executes the command without any shell;
6. puts the child in a dedicated process group (a new session via `setsid`);
7. connects to the per-user daemon, starting it if necessary;
8. verifies the daemon identity through the Unix socket (peer UID);
9. registers the job with the daemon (or continues locally if registration fails);
10. relays data between the original terminal, the child PTY, and the daemon;
11. forwards terminal signals (`SIGINT`, `SIGQUIT`, `SIGTERM`, `SIGHUP`,
    `SIGTSTP`, `SIGCONT`, `SIGWINCH`) to the child's process group;
12. restores the original terminal state on every exit path (RAII guard);
13. reports the final status to the daemon and flushes it;
14. exits with the child's exit code.

The child is a session leader with its own process group so that control
actions target the whole group, not just the immediate child.

With `jobwrap run -d`, the CLI sends its native command, environment, and cwd
over the owner-only socket. The daemon validates the request, assigns a job ID,
and starts `jobwrap detached-worker JOB_ID` in `jobwrap-job-JOB_ID.service`
through `systemd-run --user`. A one-time memory-only claim transfers the caller
context to that worker. Pending claims are bounded and expire. Environment
values are not passed in the unit command line or persisted in job metadata.

The worker resolves its daemon paths before restoring the caller environment,
then creates the normal PTY and registers the assigned ID. The launch command
returns only after registration is observed. On registration failure the worker
terminates and reaps its child; it does not leave an unmonitored detached job.
The service manager owns and reaps the worker. The original agent's process or
cgroup teardown cannot kill that independent service. No double-fork or
foreground fallback is used when the user manager is unavailable.

The worker and workload survive daemon shutdown, but the existing socket
protocol does not reconnect/adopt them. A restarted daemon reports those records
as disconnected; use the named user service to inspect or stop the workload.

## Process identities

Plain integers are never passed around as PIDs. Distinct newtypes are used:

```rust
struct ProcessId(i32);
struct ProcessGroupId(i32);
struct SessionId(i32);
struct JobId(Ulid);
```

## Job state machine

Job state is an explicit enum validated centrally (`JobState::transition`):

```text
Registering -> Running
Running     -> Stopped
Running     -> Exited
Running     -> Signaled
Running     -> Disconnected
Stopped     -> Running
Stopped     -> Signaled
Disconnected -> Running / Lost / Exited / Signaled
```

## Daemon model

The daemon runs as the invoking user, never as root. Runtime files:

```text
$XDG_RUNTIME_DIR/jobwrap/
  daemon.sock   (mode 0600)
  daemon.pid
  daemon.lock
  daemon.log    (the daemon's captured stderr; truncated on each start)
```

Persistent state:

```text
$XDG_CONFIG_HOME/jobwrap/config.toml
$XDG_STATE_HOME/jobwrap/jobwrap.db
$XDG_STATE_HOME/jobwrap/logs/{job_id}.terminal
$XDG_DATA_HOME/jobwrap/web/
```

`jobwrap` auto-starts `jobwrapd` under a lock, waits for its socket, and
verifies the socket owner's UID before trusting it. The daemon can also be
started directly with `jobwrap daemon start` (background) or
`jobwrap daemon start --foreground` (attached to the terminal), and
restarted with `jobwrap daemon restart` to pick up configuration changes.
When auto-start fails, the daemon's captured output is included in the
error. A systemd user unit for the daemon is provided but not required.
Detached jobs require a working user service manager even if the daemon was
started directly; each detached job gets its own independent transient unit.

## Crates

| Crate            | Responsibility                                                |
|------------------|--------------------------------------------------------------|
| `jobwrap-core`   | Domain types, state machine, authorization vocabulary (no I/O)|
| `jobwrap-config` | TOML models, defaults, profiles, precedence, validation      |
| `jobwrap-protocol`| Typed wire messages and the length-prefixed frame codec      |
| `jobwrap-pty`    | PTY allocation, process groups, terminal guards, relay loop   |
| `jobwrap-store`  | SQLite repositories, migrations, append-only logs, retention |
| `jobwrap-auth`   | Argon2id hashing, tokens, sessions, rate limiting, scopes    |
| `jobwrap-web`    | Axum routes, middleware, WebSockets, embedded browser assets |
| `jobwrap-daemon` | Composition root, live registry, wrapper connections         |
| `jobwrap-cli`    | Argument parsing, wrapper mode, administrative subcommands   |

## Configuration precedence

Lowest to highest:

```text
built-in defaults < user configuration
```

Profile access values record a `ValueSource` so `jobwrap config explain` can
report where they came from. Project and environment layers are not currently
implemented.

## Protocol

Frames are 4-byte big-endian length prefixes followed by JSON. Binary payloads
are base64 strings for terminal I/O. Detached native command/environment strings
use byte arrays so Linux non-UTF-8 values are preserved. All messages are typed
enums; there are no untyped
dictionaries in the protocol.
