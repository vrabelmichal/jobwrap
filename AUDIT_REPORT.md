# Safety and usability audit

Date: 2026-09-09

## Outcome

The supported `jobwrap COMMAND` foreground-wrapper path is ready for local
use. Daemon/API process creation and executable help probes now default to off.
Incomplete launch modes fail before starting a process instead of pretending
to work safely.

## High-impact findings fixed

- Removed the daemon-managed launch implementation that dropped child handles,
  leaked zombies, recorded a process group it had not created, and never
  released launch capacity.
- Disabled existing-terminal delivery that performed a truncating write to a
  client-supplied filesystem path.
- Made daemon shutdown obtain its PID from a verified live Unix connection;
  stale PID files can no longer signal unrelated processes, and the CLI no
  longer unlinks a live daemon socket.
- Added server-side `SO_PEERCRED` validation. Claimed UID/PID values are checked
  against the kernel-reported peer and registration ownership is derived from
  the peer.
- Rejected symlink/non-file database and log targets. New log files use
  `O_NOFOLLOW`, `0600`, and create-new semantics rather than truncating an
  existing path.
- Enforced 512 MiB per-job (default) and 4 GiB aggregate log limits. Replaced
  unbounded wrapper output queues with bounded backpressure.
- Fixed ignored CSRF failures, WebSocket origins, and strict same-origin
  validation for browser state changes.
- Fixed HTML/attribute injection in rendered job pages and lists.
- Required output permission for live WebSockets; status permission no longer
  exposes output or sensitive command/working-directory fields.
- Bounded browser terminal text, WebSockets, Unix connections, browser
  sessions, stored probes, probe concurrency, documentation subprocess time,
  and subprocess output.
- Bounded in-memory jobs, pending launches, registered terminals, detached
  terminal children, idempotency entries, and history query result sets.
- Added strict size and NUL validation for wrapper registrations, launch
  requests, terminal registrations, and probe requests.
- Reworked probe output capture so full stdout/stderr pipes cannot deadlock the
  target, and terminate the complete probe process group on every exit path.
- Reap terminal-emulator children through one bounded worker instead of
  dropping child handles and accumulating zombies.
- Limited static shebang inspection to 8 KiB of regular files; FIFOs/devices
  are never opened for content inspection.
- Fixed completed-job history and log access after daemon restart.
- Routed signals through the connected wrapper and rejected disconnected jobs,
  avoiding direct use of potentially reused stored process-group IDs.
- Made `--no-record` actually disable recording and `--no-web` avoid daemon
  registration. Unsupported `--detach` now errors before starting a child.
- Fixed `logs --offset`, job-state rendering, config provenance display,
  configured password rate-limit windows, idempotency scope/reuse, and several
  silent persistence/deletion failures.

## Deliberately unavailable

- Managed daemon/API launch needs real PTY capture, child reaping, state
  supervision, and bounded output before it can be enabled.
- Existing-terminal launch needs a shipped, authenticated cooperative shell
  channel. Filesystem-path delivery is not acceptable.
- Launch preview/confirmation is not implemented. With the safe default
  `require_preview = true`, launch execution fails closed.
- Help probes have limited environment/process controls but no filesystem,
  syscall, or fork containment. They remain opt-in and should be used only for
  trusted targets.
- `--detach`, interactive re-attach, automatic retention cleanup, project
  config layers, and environment config overrides remain unimplemented and are
  documented as such.

## Verification

- `cargo fmt --all -- --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace --all-targets`
- `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`
- project-local daemon/CLI smoke test (startup, status, wrapped command,
  history/log retrieval, and clean shutdown)
