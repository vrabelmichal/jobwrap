---
name: jobwrap
description: Run, monitor, and control Linux jobs with jobwrap, including detached servers and tasks that must survive an agent session. Use for servers, large uploads, lengthy calculations, and processing tasks needing recorded output and scoped control.
---

# jobwrap

The main intention of jobwrap is to support long-duration processing tasks:
servers, large uploads, lengthy calculations, batch processing, and similar
work. It wraps a locally launched Linux command in a pseudo-terminal (PTY),
preserves terminal interaction, and exposes job status, recorded output, and
authorized control through a per-user daemon. The upload or calculation is
performed by the wrapped program; jobwrap provides monitoring and control.

Read [references/cli.md](references/cli.md) for local execution, daemon management,
configuration, and credentials. Read [references/api.md](references/api.md) for
HTTP automation, byte-offset log retrieval, WebSocket monitoring, and control.
These references ship with the skill and do not require the source repository.

## Workflow for long-running work

1. Check whether the intended job already exists before starting another upload
   or calculation. Use the CLI as the local Unix owner; use a scoped bearer token
   for API automation. Confirm jobwrap is available with `jobwrap --version`.
2. Start the authorized executable once with `jobwrap run ... -- COMMAND`, a
   descriptive name, and the appropriate profile. Preserve its cwd and arguments.
   Version 0.2 requires `run`: bare executable names and the old `wrap` form are
   rejected. `jobwrap show JOB_ID` and `jobwrap status JOB_ID` are read-only queries.
3. Use `jobwrap run --detach --name NAME --profile private -- COMMAND` (or `-d`)
   for servers and tasks that must survive the launching agent. It returns after
   registration; a separate systemd user service owns the wrapper outside the
   agent's process tree and cgroup. A working user manager and `systemd-run` are
   required. An unavailable manager causes an error. Foreground `run` still
   requires keeping the execution session alive for the whole workload.
4. Save the job ID, URL, and `jobwrap-job-JOB_ID.service` name from the banner.
   Confirm workload readiness separately; registration is not readiness or
   completion. A startup timeout may have an uncertain outcome: inspect the
   reported ID/service before retrying. Foreground registration failure can
   leave the child running. Do not duplicate a workload after monitoring failure.
5. Monitor state and incremental output using bounded requests. For lengthy work,
   space polls appropriately (for example, 15–60 seconds, adapted to the task)
   or use live WebSocket events. Lack of output alone does not mean a job is stuck.
6. Report completion using the child's exit status and any task-specific evidence
   such as upload verification or calculation artifacts. An accepted signal is
   not confirmation that the process has exited. `disconnected` and `lost` mean
   monitoring is uncertain, not that the workload succeeded or can safely restart.

## Operational constraints

- Commands run directly, without an implicit shell. Pass arguments separately;
  pipes, redirects, variable expansion, and shell operators require an explicit
  shell chosen for the user's task.
- Use `--profile private` for sensitive workloads. The default `standard` profile
  exposes status and output publicly to clients that can reach the server.
  Output and arguments can contain secrets; protect tokens and avoid echoing them
  into recorded terminal output.
- Sending input can trigger application actions. Send input or signals only
  within the user's authorized task. Signals affect the child's process group;
  use graceful interruption/termination before forced killing when appropriate.
- Output recording is bounded: by default 512 MiB per job and 4 GiB aggregate.
  Recording can stop while processing continues. Logs are not guaranteed to be
  a complete artifact store; preserve workload results separately.
- `--detach` preserves the caller environment and cwd, supports `--no-record`,
  and conflicts with `--no-web`. Keep the workload's files available; detaching
  does not preserve a temporary report directory or agent worktree from cleanup.
- Detached services survive daemon shutdown, but wrapper reconnection is not
  implemented. A restarted daemon marks their records `disconnected`; recording
  and control through it cease. Inspect the named service with `systemctl --user
  status` or `journalctl --user -u`; stop it with `systemctl --user stop` only
  when the task authorizes termination. User-manager shutdown, logout policies,
  reboot, and explicit stops can end jobs; detaching is not boot persistence.
- `attach` and `logs -f` are not implemented. HTTP API
  launch is experimental, disabled by default, and only supports an explicitly
  enabled new-terminal path. Do not enable launch or weaken access settings just
  to work around a failed request.
- Use existing configuration and credentials. Installing this skill does not
  install jobwrap or authorize credential creation, configuration changes,
  network exposure, record deletion, or unrelated workload execution.

These instructions describe the current implementation. If the installed build
differs, inspect its `--help` and `/api/v1/server` response before relying on new
features; do not infer functionality from an option name alone.
