# Changelog

All notable changes to jobwrap are documented in this file.

## [Unreleased]

### Added

- Web interface: a "new job" page at `/jobs/new` (linked from the jobs list)
  so logged-in users can create processes from the browser. It submits the
  same `POST /api/v1/launch` API as the CLI, submits arguments as a
  structured list (one per line, never shell-parsed), generates a fresh
  idempotency key per attempt, and redirects to the job page once the new
  terminal's wrapper has registered the job. When daemon-side creation is
  disabled or fails closed (`[launch] enabled`, `require_preview`), the page
  shows the responsible configuration instead of the form.
- The job details page now shows the recorded command, launch metadata, process
  identifiers, terminal/log metadata, and a permission-aware live Linux
  process snapshot (state, PPID, threads, memory, open file descriptors, and
  disk I/O) from `/proc`.
- `jobwrap daemon start --foreground` runs the daemon attached to the
  current terminal (Ctrl+C stops it) instead of in the background.
- `jobwrap daemon start` prints the web interface URL after starting.
- `server.bind = "tailscale"` binds only the `tailscale0` interface, making
  the web interface reachable from other machines in the tailnet without
  exposing it to the local network or the internet. `bind` now also accepts
  a comma-separated list of targets (for example `loopback,tailscale`), and
  the daemon binds one listener per target. Other non-loopback binds remain
  refused (as documented in the security model), and unrecognized bind
  values are now rejected at configuration validation instead of failing
  later at bind time.

- `jobwrap daemon restart` stops the running daemon, waits for it to
  release its socket, and starts a new one, so configuration and binary
  changes take effect with a single command.
- `jobwrap daemon stop` now waits for the daemon to exit and reports
  `not running` (exit 0) when no daemon is up, instead of failing.

### Fixed

- Daemon startup failures are now visible: `jobwrap` captures the daemon's
  stderr in `daemon.log` under the runtime directory, fails fast when
  `jobwrapd` exits during startup, and includes the captured output
  (for example a busy HTTP port) in the error instead of waiting ten
  seconds for a socket that never appears.
- `server.public_base_url` now defaults to `http://{bind}:{port}` instead
  of a hardcoded `http://127.0.0.1:8765`, so changing `server.port` no
  longer leaves `jobwrap daemon start`, `jobwrap open`, and related URLs
  pointing at the old port. An explicitly configured value is still kept.

### Changed

- The job details page is reorganized into overview, process, command,
  terminal-output, and signal-control sections. The browser's live-output
  connection is labeled separately from the job state instead of appearing as
  an unexplained `disconnected` status.

## [0.1.0] - 2026-08-01

### Added

- `jobwrap COMMAND...` wrapper mode with a full pseudo-terminal:
  - interactive input/output, colors, progress bars, prompts;
  - terminal resize propagation;
  - Ctrl+C / Ctrl+Z / resume on the child's process group;
  - RAII terminal restoration on every exit path;
  - correct child exit-code propagation.
- Per-user `jobwrapd` daemon with:
  - auto-start under a lock;
  - private Unix socket with `0600` permissions and peer-UID verification;
  - job registry with an explicit, centrally validated state machine.
- Typed wire protocol (length-prefixed JSON, base64 binary payloads).
- SQLite persistence for job metadata, events, tokens, sessions, and audit.
- Append-only terminal output logs with per-job byte limits.
- Loopback HTTP API (`/api/v1`) and WebSocket live output streaming.
- Authentication:
  - Argon2id password hashing with a non-echoing prompt;
  - HttpOnly + SameSite=Strict browser sessions;
  - scoped API tokens stored hashed, shown once;
  - password attempt rate limiting.
- Authorized process control: input, SIGINT, SIGTERM, SIGSTOP, SIGCONT,
  SIGKILL, delete, with a single authorization entry point.
- Embedded browser interface (no build pipeline).
- Configuration with profiles, precedence, provenance, and validation.
- CLI subcommands: `list`, `show`, `logs`, `signal`, `stop`, `open`,
  `daemon`, `config`, `auth`, `token`.
- PTY integration tests covering the plan's acceptance criteria.

### Non-goals for 0.1

- arbitrary command execution through the HTTP API;
- internet-facing TLS server;
- Windows/macOS support;
- full tmux/screen replacement.
