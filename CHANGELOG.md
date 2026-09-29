# Changelog

All notable changes to jobwrap are documented in this file.

## [Unreleased]

### Added

- The job details page now shows the recorded command, launch metadata, process
  identifiers, terminal/log metadata, and a permission-aware live Linux
  process snapshot (state, PPID, threads, memory, open file descriptors, and
  disk I/O) from `/proc`.
- `jobwrap daemon start --foreground` runs the daemon attached to the
  current terminal (Ctrl+C stops it) instead of in the background.
- `jobwrap daemon start` prints the web interface URL after starting.

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
