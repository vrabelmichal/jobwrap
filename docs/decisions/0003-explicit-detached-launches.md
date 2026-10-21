# ADR-0003: Explicit launches and independent detached services

Date: 2026-10-21

## Status

Accepted. Amends the process-creation scope of ADR-0001.

## Context

A preview server lost its wrapper when its launching agent session ended.
The daemon had no final child status and could only report `disconnected`.
Foreground wrapping cannot promise survival when the agent kills its process
tree or cgroup. Separately, `jobwrap status JOB_ID` was treated as an executable
launch because unrecognized first words implicitly selected wrapper mode.

## Decision

Require `jobwrap run [OPTIONS] -- COMMAND [ARGS...]` for all launches. Unknown
subcommands, bare executable names, and the old `wrap` form are errors before
launch. `show` (alias `status`) remains a read-only query. The executable stays
named `jobwrap`; no `wrap`, `jwrap`, or `-R` launch alias is introduced.

Implement `run --detach` (`-d`) through an owner-authenticated local socket
request and a separate transient systemd user service. The service manager
starts a fixed worker that claims the requested caller context once from daemon
memory and uses the existing PTY, recording, and control paths. The CLI confirms
registration before returning. An unavailable user manager causes an error;
there is no weaker process-tree-only fallback. The optional HTTP launch API
remains disabled by default and does not expose this local operation.

Native argument, cwd, and environment bytes are preserved. Environment values
are excluded from persisted metadata, debug rendering, and unit arguments.
Detached registration failure terminates and reaps the child. Startup timeouts
include the job/service identity so callers can inspect before retrying.

## Consequences

Detached jobs survive terminal/agent process and cgroup teardown. They require
`systemd-run` and a running user service manager, and they can still be stopped
by explicit control, manager shutdown, logout policy, or reboot. No automatic
workload restart or boot persistence is configured.

Detached workers survive daemon shutdown. The current protocol cannot reconnect
or adopt them, so a restarted daemon marks their records disconnected and the
user service must be inspected/stopped directly. This limit is documented.

The CLI syntax change is intentional and ships in 0.2.0. Documentation, skills,
manpages, shell completions, and user-facing guidance must use `run` explicitly.

The choice of transient services follows
[systemd-run's service-manager ownership model](https://www.freedesktop.org/software/systemd/man/latest/systemd-run.html).
Using a new process group or session would leave the process in the agent's
cgroup, which [systemd can terminate as a whole](https://www.freedesktop.org/software/systemd/man/latest/systemd.kill.html).
