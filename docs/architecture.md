# Architecture

jobwrap wraps a locally launched Linux process in a pseudo-terminal and exposes
secure monitoring and narrowly authorized control through a per-user daemon.

## Components

```text
GNOME Terminal / shell
   |
   +-- jobwrap COMMAND ARGS          (/usr/bin/jobwrap)
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
  with the daemon, and relays terminal traffic. It is the only component that
  ever creates processes.
* **`jobwrapd`** — a per-user daemon providing the web interface, HTTP API,
  authentication, job registry, log storage, and access control. Experimental
  launch/probe features are disabled by default; the only opt-in launch mode
  currently available starts the fixed `jobwrap attach-launch` helper in a
  supported terminal emulator.

## Process model

When invoked, `jobwrap`:

1. parses only its own arguments (before `--` or an unrecognized first word);
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
```

Persistent state:

```text
$XDG_CONFIG_HOME/jobwrap/config.toml
$XDG_STATE_HOME/jobwrap/jobwrap.db
$XDG_STATE_HOME/jobwrap/logs/{job_id}.terminal
$XDG_DATA_HOME/jobwrap/web/
```

`jobwrap` auto-starts `jobwrapd` under a lock, waits for its socket, and
verifies the socket owner's UID before trusting it. A systemd user unit is
provided but not required.

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
are base64 strings. All messages are typed enums; there are no untyped
dictionaries in the protocol.
