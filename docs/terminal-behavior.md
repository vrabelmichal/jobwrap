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

Each worker belongs to `jobwrap-job-JOB_ID.service`. A daemon restart leaves
that service alive, but there is no wrapper reconnection/adoption: recording
and daemon control cease and the record becomes disconnected. Use
`systemctl --user status jobwrap-job-JOB_ID.service` or
`journalctl --user -u jobwrap-job-JOB_ID.service` for inspection, and
`systemctl --user stop jobwrap-job-JOB_ID.service` for termination. A user
manager shutdown, reboot, logout policy, or explicit service stop can end the
job; detached mode does not configure login lingering or restart-on-boot.

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

There is no wrapper reconnection or adoption of an existing child. If the
wrapper was the sole owner of the PTY master, its exit loses that monitoring
channel. A surviving child can still be observed through its own log and Linux
process information. Do not relaunch a workload solely because its jobwrap
record is disconnected, and do not send signals to saved PIDs without verifying
their current identity.
