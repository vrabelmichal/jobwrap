# Terminal behavior

The wrapped process runs inside a pseudo-terminal so it behaves indistinguishably
from a normal foreground command.

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

## Known limitations (version 1)

* Nested job control inside an interactive shell (`jobwrap bash`) has edge
  cases; the primary supported workload is
  `jobwrap executable arguments...`.
* When stdin is a pipe that reaches EOF, the child does not see an EOF on its
  PTY input.
