---
name: jobwrap
description: Run, monitor, and control long-running Linux commands with jobwrap through its CLI, HTTP API, and live output stream. Use for large uploads, lengthy calculations, and other processing tasks that need progress tracking and scoped control.
---

# jobwrap

The main intention of jobwrap is to support long-duration processing tasks:
large uploads, calculations that take a long time, batch processing, and similar
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
2. Start the authorized executable once, with a descriptive name and an
   appropriate access profile. Prefer the supported `jobwrap wrap ... -- COMMAND`
   path. Preserve the workload's working directory and arguments.
3. Keep the launch terminal or execution session alive. **jobwrap currently does
   not implement `--detach`** and does not guarantee survival of terminal closure
   or an agent execution timeout. Use an available persistent terminal/session
   mechanism when needed; do not assume the daemon owns the wrapped process.
4. Save the registered job ID and URL from the startup banner. Registration can
   fail while the child continues running: a monitoring failure is not permission
   to rerun the workload. Inspect the original session first.
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
- `attach`, `logs -f`, and detached wrapping are not implemented. API/daemon
  launch is experimental, disabled by default, and only supports an explicitly
  enabled new-terminal path. Do not enable launch or weaken access settings just
  to work around a failed request.
- Use existing configuration and credentials. Installing this skill does not
  install jobwrap or authorize credential creation, configuration changes,
  network exposure, record deletion, or unrelated workload execution.

These instructions describe the current implementation. If the installed build
differs, inspect its `--help` and `/api/v1/server` response before relying on new
features; do not infer functionality from an option name alone.
