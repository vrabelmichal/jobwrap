# jobwrap

Wrap a locally launched Linux process and securely expose monitoring and
narrowly authorized control.

`jobwrap` runs a command in a pseudo-terminal so it behaves exactly like a
normal foreground command, while a per-user daemon provides a web interface,
HTTP API, and scoped control.

```text
jobwrap python3 run_analysis.py --config analysis.yaml
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
jobwrap top                  # any command; colors and Ctrl+C work
```

In another terminal:

```sh
jobwrap list                 # list jobs
jobwrap show <JOB_ID>        # details
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
* daemon/API process creation and executable help probes are disabled by default;
* wrapped commands are never run through an implicit shell;
* only named signals are exposed;
* passwords and tokens are stored hashed;
* log growth, protocol frames, WebSockets, and daemon socket connections are bounded.

The supported process path is `jobwrap COMMAND`. The experimental `jobwrap
launch` API is opt-in; managed and existing-terminal modes deliberately fail
closed until their lifecycle/control integrations are complete. See
`docs/security-model.md` and `AUDIT_REPORT.md`.

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
