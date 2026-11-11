# Terminal behavior

The wrapped process runs inside a pseudo-terminal. In foreground mode it
behaves like a normal terminal command. Detached mode keeps that PTY and daemon
control while running independently of the launching terminal.

## What works

* output appears in the original terminal;
* colors and progress bars work (raw bytes pass through unchanged);
* interactive prompts work;
* terminal resize information reaches the child (`SIGWINCH` is forwarded and
  the PTY window size is updated);
* Ctrl+C interrupts the child's process group;
* Ctrl+Z suspends the child's process group and suspends the wrapper so the
  shell regains control; resume (`fg`/`SIGCONT`) forwards continue;
* terminal input reaches the child;
* the wrapper exits with the child's exit code;
* the daemon and web clients see the same output;
* authorized clients can send input or named signals.

## Terminal data is bytes

Terminal streams can contain arbitrary byte sequences, not just UTF-8. Output
is treated as bytes end to end: the PTY relay reads raw bytes, the protocol
base64-encodes them, and the log stores them verbatim.

## Terminal restoration

The wrapper captures the terminal state before starting the child and restores
it on every exit path:

* normal exit;
* child crash;
* wrapper `SIGTERM`;
* daemon communication failure;
* registration failure after the child starts;
* terminal disconnect.

Restoration is managed by an RAII guard (`TerminalGuard`).

## Relay mode

While the child runs, the wrapper's own terminal is put into relay mode: echo,
canonical input, flow control and output processing are disabled so the PTY
slave performs them, avoiding double echo. `ISIG` stays on so Ctrl+C/Ctrl+Z
still generate signals to the wrapper, which forwards them to the child.

## Signals and process groups

Signals target the child's process group, not just the immediate child:

```text
SIGINT  -> process group
SIGTERM -> process group
SIGSTOP -> process group
SIGCONT -> process group
SIGKILL -> process group
```

## Known limitations

* Nested job control inside an interactive shell (`jobwrap run -- bash`) has edge
  cases; the primary supported workload is
  `jobwrap run -- executable arguments...`.
* When stdin is a pipe that reaches EOF, the child does not see an EOF on its
  PTY input.

## Wrapper lifetime and disconnection

`jobwrap run` runs in the foreground by default. An agent
execution session must remain alive for the entire command. Jobwrap has no
internal job duration limit, but it cannot prevent a terminal, execution tool,
or operating system from terminating the wrapper.

Use `jobwrap run --detach -- COMMAND` (or `-d`) for servers and tasks that must
outlive the launching agent. The local daemon starts a fixed worker in a
separate systemd user service. A new session or double-fork alone would not
escape an agent's cgroup cleanup; the independent service manager does.
Detached mode requires `systemd-run` and a working user manager and fails
explicitly when they are unavailable. It does not enable the HTTP launch API.

The detached worker preserves command arguments, cwd, and environment, including
native non-UTF-8 bytes, creates the normal PTY, and continues recording output
and accepting authorized input/signals. It does not relay the old terminal's
stdin or resize events. `--no-record` is supported; `--no-web` conflicts with
detached mode. The CLI exits zero once the job is registered, not when the child
finishes. Inspect `show`/`status` and workload readiness separately. A startup
timeout can have an uncertain outcome; inspect the reported job ID and service
before retrying. Registration failure terminates and reaps the detached child.

Each detached worker belongs to `jobwrap-job-JOB_ID.service`. Newly built
workers reconnect after daemon restart, authenticate with their socket peer,
boot ID and process start time, and replay output by acknowledged sequence.
Input and signals become available again when connected. The CLI tries to start
its daemon in an independent transient user service too; if the service manager
is unavailable, it warns before falling back to a session-dependent daemon.
Explicit `jobwrap daemon start --foreground` remains attached.

Workers drain the PTY into a private, bounded recovery journal independently of
the daemon. Recovery output is capped at 512 MiB of framed data per job (including base64
overhead) and 4 GiB across recovery journals, in addition to the normal terminal
logs. A bounded lifecycle reserve allows final results after output fills. Once full, further output is
not recorded and truncation is reported. Final status remains recordable. Normal
log pruning removes the matching journal. `--no-record` journals lifecycle
metadata only; live output during an outage cannot be replayed in that mode.
A worker waits briefly for final acknowledgement; if the daemon is still absent,
it exits with its child's status. The daemon recovers completed journals on
startup and periodic reconciliation, even if the worker has gone away.

These guarantees require protocol version 2 in both binaries and newly launched
workers. Upgrade the CLI and daemon together. Already-running old workers cannot
acquire reconnection support. For those, inspect the named unit with
`systemctl --user status jobwrap-job-JOB_ID.service` or its user journal.
A user-manager shutdown, reboot, logout policy, or explicit service stop can end
the job; detached mode does not configure login lingering or restart-on-boot.

If writes to the original terminal fail, the wrapper continues draining the PTY
and recording output through the daemon. Forwarding TERM/HUP also keeps the
reader active until PTY EOF so that child signal handlers can finish writing.
These behaviors do not protect against the wrapper itself being killed.

When the daemon loses the wrapper connection without a final child status, it
records `disconnected`, a timestamped disconnection event, an audit event with
the observed connection-loss reason, and a state-change event. It also pushes
the state change to connected web clients. A socket EOF identifies loss of the
connection, not the signal or external action that caused the wrapper to exit.
The finish time remains unknown; `disconnected` does not establish workload
failure or completion.

There is no adoption of a child whose wrapper has died. If the
wrapper was the sole owner of the PTY master, its exit loses that monitoring
channel. A surviving child can still be observed through its own log and Linux
process information. Do not relaunch a workload solely because its jobwrap
record is disconnected, and do not send signals to saved PIDs without verifying
their current identity.

## Browser output

The browser renders ANSI SGR foreground/background colors (16-color, 256-color
and RGB), bold, dim, italic, underline, reverse and strike-through. It handles
common line-editing sequences, carriage returns and backspaces for progress
output. UTF-8 and escape sequences may cross network-frame boundaries. Workload
text is escaped; OSC titles, hyperlinks and other terminal side effects are not
executed. This is a log viewer, not a full-screen terminal emulator.

The browser subscribes before catching up recorded history, reads all history
pages by byte offset, and reconnects with backoff. Stored byte offsets remove
overlap between live and history output; a lagged broadcast requests catch-up.
The visible scrollback is bounded to about 2 MiB of rendered markup and 16,384
cells per line. The raw recorded log remains available through the CLI/API,
subject to recording limits. Programs must emit ANSI colors themselves;
jobwrap does not override `NO_COLOR` or force a program's color policy.
As with any PTY, stdout and stderr share one ordered terminal stream.

## Opt-in reboot recovery for named servers

`jobwrap server-unit --name review -- .venv/bin/python -m http.server 8080`
prints a persistent systemd user service definition. It does not write a unit,
enable it, or start a server. Review the definition and save it in your user
unit directory, then use the enable command printed in its comments if desired.
The definition runs foreground jobwrap inside the independent service, preserves
selected Python/path activation settings, restarts on failure, and starts with
the user manager after login/reboot. It deliberately omits arbitrary environment
variables and credentials; configure required credentials separately.

Use stable executable, environment and working-directory paths. A deleted
worktree or unmounted data drive can still prevent startup. Boot without login
requires the user's separately configured lingering policy. One-off detached
computations retain their existing no-restart behavior. Each service invocation
registers a new job; the historical results remain separate.

`doctor JOB_ID` now includes the last observed wrapper contact/output times and
recorded disconnection reason. These are observations, not an inference that a
quiet workload is stuck or that socket EOF proves an external kill signal.
