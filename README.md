# jobwrap

Wrap a locally launched Linux process and securely expose monitoring and
narrowly authorized control.

`jobwrap` runs a command in a pseudo-terminal so it behaves exactly like a
normal foreground command, while a per-user daemon provides a web interface,
HTTP API, and scoped control.

```text
jobwrap run -- python3 run_analysis.py --config analysis.yaml
```

The wrapper prints registration information, then the process runs as usual —
colors, progress bars, interactive prompts, resize, Ctrl+C and Ctrl+Z all work.
The exit code is the child's.

## Quick start

```sh
cargo build --release
export PATH="$PWD/target/release:$PATH"

# start the daemon and its web interface (it prints the URL):
jobwrap daemon start
```

Or skip that step entirely: `jobwrap` starts the daemon automatically the
first time it needs one. Just run a command:

```sh
jobwrap run -- top           # foreground; colors and Ctrl+C work
```

In another terminal:

```sh
jobwrap list                 # list jobs
jobwrap show <JOB_ID>        # details
jobwrap status <JOB_ID>      # alias for show; never starts a job
jobwrap signal <JOB_ID> int  # send SIGINT
jobwrap daemon restart       # apply a config change
jobwrap daemon stop          # stop the daemon
```

Open the printed web URL for live output. When daemon-side process creation
is enabled in the configuration (it is off by default; see
`docs/configuration.md`), the web interface also offers a "new job" form at
`/jobs/new`. Use
`jobwrap daemon start --foreground` to keep the daemon attached to the
terminal instead of the background.

## Detached servers and agent tasks

Use detached mode when the job must outlive its launching terminal or AI agent:

```sh
jobwrap run --detach --name report-server --profile private -- \
  python3 -m http.server 8080 --bind 127.0.0.1
# --detach can also be written -d.
```

The CLI returns the registered job ID and URL. The wrapper runs in its own
systemd user service, outside the launcher's process tree and cgroup, so agent
process cleanup does not terminate it. Detached mode requires `systemd-run`
and a working systemd user manager; an unavailable manager produces an error.
It preserves the caller's executable, arguments, environment, and working
directory. Logs, web output, input, and signals still use the daemon.

Check `jobwrap show JOB_ID` and the server's readiness endpoint before reporting
that it is ready. A successful detached launch confirms registration, not
application readiness or eventual success. Use `jobwrap stop JOB_ID` to request
termination, then check its final state.

Detached services survive a jobwrap daemon restart, but wrapper reconnection
is not implemented: those records become `disconnected`, and recording/control
through the restarted daemon are unavailable. Inspect the service with
`systemctl --user status jobwrap-job-JOB_ID.service`; use
`systemctl --user stop jobwrap-job-JOB_ID.service` to stop it if necessary.
User-manager shutdown, logout policies, reboot, or an explicit service stop can
still terminate it. Detached mode is not automatic restart or boot persistence.

## Explicit launch syntax in 0.2

Launch commands use `jobwrap run [OPTIONS] -- COMMAND [ARGS...]`. The old bare
`jobwrap COMMAND`, `jobwrap -- COMMAND`, and `jobwrap wrap` forms are rejected.
Unknown subcommands and typos fail before creating a job. There is no `wrap`
or `jwrap` executable alias. For example, running an executable named `status`
requires `jobwrap run -- status`; `jobwrap status JOB_ID` inspects a job.

## Version and build information

`jobwrap --version` and `jobwrapd --version` report the application version,
source Git commit, and whether the build included uncommitted changes. Use
`-V` for the short version. `GET /api/v1/server` exposes the same information
as `version`, `git_commit`, `build_dirty`, and `host_name`; every web page
starts its tab title with the hostname and shows the hostname in the header,
alongside its build information footer. Builds from source archives without
Git metadata report the commit as `unknown`.

The workspace version is the source of truth: application `0.2.0` is released
as GitHub tag `v0.2.0` and Debian packages `0.2.0-1`, where `-1` is the Debian
packaging revision.

## Layout

```text
crates/jobwrap-core      domain types and authorization
crates/jobwrap-config    configuration
crates/jobwrap-protocol  wire protocol
crates/jobwrap-pty       PTY and process control
crates/jobwrap-store     SQLite and logs
crates/jobwrap-auth      passwords, tokens, sessions
crates/jobwrap-web       HTTP and WebSocket
crates/jobwrap-daemon    jobwrapd
crates/jobwrap-cli       jobwrap
docs/                    architecture, security model, API, configuration
```

## Security

See `docs/security-model.md`. Highlights:

* the HTTP server binds to loopback by default;
* HTTP API process creation and executable help probes are disabled by default;
* wrapped commands are never run through an implicit shell;
* only named signals are exposed;
* passwords and tokens are stored hashed;
* log growth, protocol frames, WebSockets, and daemon socket connections are bounded.

The supported process path is `jobwrap run -- COMMAND`, with `-d` for detached
jobs through the owner-only local socket. The experimental `jobwrap
launch` API is opt-in; managed and existing-terminal modes deliberately fail
closed until their lifecycle/control integrations are complete. See
`docs/security-model.md` and `AUDIT_REPORT.md`.

## Debian packages

Build application and optional Codex skill packages with
`python3 packaging/build-deb.py`; use `--codex-skills no` to build only the
application. Manpages and Bash/Zsh/Fish completions are included. Install the
separate `jobwrap-codex-skills` package only if you want the skill available to
all machine users. See [Debian packaging](docs/debian-packaging.md) for build
prerequisites, installation choices, unattended commands, and verification.

Publish downloadable packages on GitHub with
`python3 packaging/release-github.py --maintainer 'Name <your-real-email>'`.
Use `--dry-run` for a local preview or `--draft` to upload a draft release.
See [GitHub releases](docs/github-releases.md) for the release workflow.

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
```

The workspace targets Rust 1.72; all dependencies are pinned in `Cargo.lock`.

## License

MIT (see `LICENSE`).
