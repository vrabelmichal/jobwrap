# Recommended implementation direction

Use Rust and build the project as two cooperating executables:

```text
/usr/bin/jobwrap
/usr/libexec/jobwrap/jobwrapd
```

`jobwrap` is the command users type in an ordinary terminal.

`jobwrapd` is a per-user daemon providing the web interface, API, authentication, job registry, log storage, and access control.

The wrapped process remains connected to the terminal through a pseudo-terminal, so it behaves like a command launched normally.

The daemon communicates with each wrapper through a private Unix-domain socket.

Rust is the best fit here because it provides compile-time memory-safety guarantees without garbage collection, strong typed error handling, good concurrency support, and a mature formatter/linter/test workflow. Rust's ownership system is specifically designed to provide memory safety without a garbage collector.

Go would make the web server somewhat simpler, but Rust is preferable because this program interacts with PTYs, Unix sockets, process groups, signals, credentials, terminal state, and potentially untrusted network clients. Those are precisely the areas where stronger type and memory guarantees are valuable.

---

## 1. Intended user experience

The primary workflow should remain almost unchanged:

```text
jobwrap python3 run_analysis.py --config analysis.yaml
```

Optionally, provide a short alias:

```text
alias jw='jobwrap'
```

Then:

```text
jw python3 run_analysis.py
```

The terminal should display something like:

```text
jobwrap: registered job 01K1N8QVCW6Y7R8WDPYVJN4R3C
jobwrap: name analysis-20260801-120531
jobwrap: web http://127.0.0.1:8765/jobs/01K1N8QVCW6Y7R8WDPYVJN4R3C
jobwrap: control remains available from this terminal

Starting analysis...
```

The process should otherwise look and behave like a normal foreground command:

- output appears in the original terminal;
- colors and progress bars work;
- interactive prompts work;
- terminal resize information reaches the child;
- Ctrl+C interrupts the wrapped process;
- Ctrl+Z suspends it;
- terminal input reaches the child;
- the wrapper exits with the child's exit code;
- the daemon and web clients can monitor the same output;
- authorized clients can send input or signals.

Useful variations:

```text
jobwrap --name galfit-run-12 python3 run.py
jobwrap --profile private ./sensitive-analysis
jobwrap --detach ./overnight-pipeline
jobwrap --no-web ./local-only-command
```

The default invocation must not require:

- creating a configuration entry;
- choosing a port;
- opening a browser;
- starting a daemon manually;
- entering a password;
- naming the job;
- selecting a profile.

Defaults should make the simplest invocation useful and secure.

---

## 2. High-level architecture

```text
┌──────────────── Existing GNOME Terminal ────────────────┐
│                                                         │
│  interactive shell                                      │
│       │                                                 │
│       └── jobwrap COMMAND ARGUMENTS                     │
│              │                                          │
│              ├── terminal/PTY relay                     │
│              │      └── child process group             │
│              │            └── COMMAND + descendants     │
│              │                                          │
│              └── private Unix socket                    │
└───────────────────────┬─────────────────────────────────┘
                        │
                        ▼
             per-user jobwrapd daemon
             ├── job registry
             ├── authorization engine
             ├── event/log store
             ├── Unix-socket API
             ├── HTTP API
             ├── WebSocket output/input
             └── browser interface
```

A major architectural rule should be:

> The web server never directly creates arbitrary processes in the initial release.

Processes are created only by `jobwrap` invoked by the local Unix user. This significantly reduces the security scope. The API can control registered processes, but it cannot initially execute an arbitrary new shell command.

Remote command creation can be considered later as a separate, explicitly enabled feature.

---

## 3. Process model

### 3.1 Wrapper lifecycle

When invoked, `jobwrap` should:

1. Parse only its own arguments before `--`.
2. Load the user configuration.
3. Determine the effective job profile.
4. Connect to the per-user daemon.
5. Start the daemon if it is not running.
6. Validate the daemon identity through the Unix socket.
7. Allocate a new pseudo-terminal.
8. Fork and execute the requested command.
9. Put the child into a dedicated process group.
10. Register the job with the daemon.
11. Relay data between:
    - the original terminal;
    - the child PTY;
    - the daemon.
12. Forward terminal signals and size changes.
13. Restore the original terminal state on every exit path.
14. Report the final status to the daemon.
15. Exit using an appropriate code derived from the child status.

### 3.2 Why a PTY is required

Using stdout/stderr pipes would be easier, but it would alter command behavior:

- output might become buffered;
- applications may disable color;
- progress bars may disappear;
- interactive prompts may fail;
- terminal dimensions would be unavailable;
- Ctrl+C and Ctrl+Z behavior would differ.

A PTY is therefore a core requirement, not an optional enhancement.

### 3.3 Foreground process groups

The child command and its descendants should have a dedicated process group. Control actions should target the group rather than only the immediate child:

```text
SIGINT  → process group
SIGTERM → process group
SIGSTOP → process group
SIGCONT → process group
SIGKILL → process group
```

This prevents a shell pipeline or worker subprocess from surviving after the main process is stopped.

The implementation must carefully distinguish:

- child PID;
- process-group ID;
- session ID;
- wrapper PID;
- daemon PID.

Use dedicated strong types rather than passing plain integers everywhere:

```rust
struct ProcessId(i32);
struct ProcessGroupId(i32);
struct SessionId(i32);
struct JobId(Ulid);
```

### 3.4 Terminal signal forwarding

The wrapper must correctly handle:

- `SIGINT`
- `SIGQUIT`
- `SIGTERM`
- `SIGHUP`
- `SIGTSTP`
- `SIGCONT`
- `SIGWINCH`
- `SIGCHLD`

Terminal restoration must be robust even when:

- the child crashes;
- the wrapper receives `SIGTERM`;
- daemon communication fails;
- registration fails after the child starts;
- the terminal disconnects;
- an internal task panics.

Use RAII guards for terminal modes and file descriptors.

### 3.5 Initial limitation

For version 1, document that job control inside a nested interactive shell may have edge cases. The primary supported workload should be:

```text
jobwrap executable arguments...
```

rather than:

```text
jobwrap bash
```

Interactive shells can be supported, but should not be the first acceptance criterion.

---

## 4. Daemon model

### 4.1 Per-user daemon

The daemon should run as the invoking user, never as root.

Suggested runtime files:

```text
$XDG_RUNTIME_DIR/jobwrap/
├── daemon.sock
├── daemon.pid
└── daemon.lock
```

Persistent files:

```text
$XDG_CONFIG_HOME/jobwrap/config.toml
$XDG_STATE_HOME/jobwrap/jobwrap.db
$XDG_STATE_HOME/jobwrap/logs/
$XDG_DATA_HOME/jobwrap/web/
```

Resolved defaults typically become:

```text
~/.config/jobwrap/config.toml
~/.local/state/jobwrap/jobwrap.db
~/.local/state/jobwrap/logs/
```

Do not place authentication secrets in `/tmp`.

### 4.2 Daemon startup

`jobwrap` should attempt to connect to the Unix socket. When unavailable:

1. acquire a daemon startup lock;
2. check again in case another wrapper started it;
3. launch `jobwrapd`;
4. wait for its socket to become ready;
5. verify that the socket owner matches the current UID;
6. continue registration.

A systemd user unit can be installed, but should not be required for basic operation.

Optional packaged unit:

```text
/usr/lib/systemd/user/jobwrapd.service
/usr/lib/systemd/user/jobwrapd.socket
```

The default approach can use systemd socket activation where available, with self-start as a fallback.

### 4.3 Unix-socket trust

For local CLI operations, authorization can use Unix peer credentials:

- obtain peer PID, UID, and GID;
- require the peer UID to match the daemon UID;
- optionally inspect the peer process where appropriate;
- never trust a caller-supplied UID.

Local owner operations through the Unix socket should not require a password.

Examples:

```text
jobwrap list
jobwrap inspect JOB_ID
jobwrap signal JOB_ID INT
jobwrap stop JOB_ID
jobwrap open JOB_ID
jobwrap token create
```

---

## 5. Job state model

Use an explicit state machine rather than independent booleans.

```rust
enum JobState {
    Registering,
    Running,
    Stopped,
    Exited { code: i32 },
    Signaled { signal: Signal },
    Disconnected,
    Lost,
}
```

Valid transitions must be enforced centrally:

```text
Registering → Running
Running     → Stopped
Running     → Exited
Running     → Signaled
Running     → Disconnected
Stopped     → Running
Stopped     → Signaled
Disconnected → Lost
Disconnected → Exited, if later reconciled
```

Avoid structures such as:

```rust
running = true
stopped = true
exited = false
```

that can represent impossible combinations.

Each job record should contain:

```rust
struct JobRecord {
    id: JobId,
    display_name: JobName,
    owner_uid: UserId,
    wrapper_pid: ProcessId,
    child_pid: ProcessId,
    process_group_id: ProcessGroupId,
    command: CommandDisplay,
    executable: PathBuf,
    arguments: Vec<OsString>,
    working_directory: PathBuf,
    profile_name: ProfileName,
    access_policy: AccessPolicy,
    state: JobState,
    started_at: DateTime<Utc>,
    finished_at: Option<DateTime<Utc>>,
    terminal: TerminalMetadata,
    log: LogMetadata,
}
```

Do not expose all of this through every API. Separate internal records from public response models.

---

## 6. Configuration design

### 6.1 Main configuration

Use:

```text
~/.config/jobwrap/config.toml
```

TOML is a good choice because Rust has strong serialization support through serde, and the format supports explicit strings, arrays, tables, integers, and booleans.

Example:

```toml
config_version = 1

[server]
bind = "127.0.0.1"
port = 8765
public_base_url = "http://127.0.0.1:8765"
open_browser_on_start = false
allow_remote_bind = false

[daemon]
auto_start = true
idle_shutdown_minutes = 0

[defaults]
profile = "standard"
allocate_pty = true
record_output = true
retain_completed_days = 30
maximum_log_bytes = 536870912
show_job_url = true
job_name_template = "{executable}-{timestamp}"

[authentication]
trust_local_owner = true
browser_session_minutes = 720
password_attempt_limit = 5
password_attempt_window_seconds = 60

[profiles.standard]
status = "public"
output = "public"
command = "authenticated"
working_directory = "authenticated"
send_input = "controller"
signal_interrupt = "controller"
signal_terminate = "controller"
signal_stop = "controller"
signal_continue = "controller"
signal_kill = "owner"
restart = "owner"
delete = "owner"

[profiles.private]
status = "authenticated"
output = "authenticated"
command = "authenticated"
working_directory = "authenticated"
send_input = "owner"
signal_interrupt = "owner"
signal_terminate = "owner"
signal_stop = "owner"
signal_continue = "owner"
signal_kill = "owner"
restart = "owner"
delete = "owner"
```

### 6.2 Configuration precedence

Use a documented and deterministic order:

```text
built-in defaults
    < user configuration
    < named profile
    < trusted project configuration
    < environment overrides
    < command-line options
```

Every resolved field should retain provenance so the diagnostics command can report where it came from:

```text
output access = public
source        = profiles.standard.output
file          = /home/michal/.config/jobwrap/config.toml:24
```

Provide:

```text
jobwrap config path
jobwrap config init
jobwrap config validate
jobwrap config effective
jobwrap config explain profiles.standard.output
```

### 6.3 Versioned configuration

Require:

```toml
config_version = 1
```

Unknown versions should fail with a clear message. Unknown fields should generally be rejected rather than silently ignored, especially for authorization settings.

Bad:

```text
Warning: ignored unknown field signal_interupt
```

Good:

```text
error: unknown configuration field `signal_interupt`
  --> /home/michal/.config/jobwrap/config.toml:31:1
   |
31 | signal_interupt = "controller"
   | ^^^^^^^^^^^^^^^^
   |
help: did you mean `signal_interrupt`?
```

Use source-span-aware TOML parsing if practical so errors can identify file locations.

---

## 7. Authentication and authorization

Authentication and authorization must remain separate concepts.

- **Authentication:** Who is making the request?
- **Authorization:** Is that identity allowed to perform this operation on this job?

### 7.1 Identity types

```rust
enum Principal {
    Anonymous,
    BrowserSession(SessionPrincipal),
    ApiToken(TokenPrincipal),
    LocalUnixUser(LocalPrincipal),
}
```

### 7.2 Access levels

Do not represent authorization as arbitrary strings after parsing.

```rust
enum RequiredAccess {
    Public,
    Authenticated,
    Controller,
    Owner,
    Disabled,
}
```

Possible role ordering:

```text
Anonymous < Authenticated < Controller < Owner
```

However, token scopes should be evaluated independently rather than assuming every controller can perform every control action.

### 7.3 Permission enumeration

```rust
enum Permission {
    ViewStatus,
    ViewOutput,
    ViewCommand,
    ViewWorkingDirectory,
    SendInput,
    SendInterrupt,
    SendTerminate,
    SendStop,
    SendContinue,
    SendKill,
    Restart,
    Delete,
    DownloadLogs,
    ModifyPolicy,
}
```

Use an authorization function with one obvious entry point:

```rust
fn authorize(
    principal: &Principal,
    job: &JobRecord,
    permission: Permission,
) -> Result<AuthorizationDecision, AuthorizationError>
```

All HTTP handlers, WebSocket actions, and local commands must call this same authorization layer.

### 7.4 Passwords

Passwords should:

- be entered through a non-echoing terminal prompt;
- be hashed with Argon2id;
- use a unique random salt;
- never be passed as command-line arguments;
- never be logged;
- never be stored in environment variables;
- never be returned by the API.

Provide:

```text
jobwrap auth set-password
jobwrap auth remove-password
jobwrap auth status
```

One global password can initially grant the Controller browser role. Job-specific passwords can be deferred until later unless there is a concrete need.

### 7.5 API tokens

Agents should use scoped tokens rather than passwords.

```text
jobwrap token create \
    --name coding-agent \
    --scope jobs:list \
    --scope jobs:status \
    --scope jobs:output \
    --scope jobs:signal:int
```

A token must support:

- random generation from a cryptographically secure source;
- display exactly once;
- hashed storage;
- scopes;
- optional job restrictions;
- expiration;
- last-used timestamp;
- revocation;
- description;
- audit identity.

Example restrictions:

```text
jobs:list
job:*:status
job:*:output
job:01K...:signal:int
```

Prefer a structured scope model internally:

```rust
struct TokenGrant {
    resource: ResourceSelector,
    permissions: BTreeSet<Permission>,
}
```

Do not implement authorization by matching raw scope strings throughout the codebase.

### 7.6 Browser sessions

After password authentication:

- issue a random opaque session ID;
- store only its hash server-side;
- use an HttpOnly cookie;
- set SameSite=Strict;
- set Secure whenever HTTPS is active;
- rotate the session after login;
- enforce expiration;
- protect state-changing browser requests against CSRF.

### 7.7 Public information

"Public" must be granular. A public status endpoint should not accidentally reveal:

- environment variables;
- access tokens embedded in arguments;
- absolute private paths;
- command-line secrets;
- terminal input;
- other users' job identities.

The recommended default is:

```text
status:            public
output:            public
command:           authenticated
working directory: authenticated
environment:       never exposed
input:             controller
SIGINT:            controller
SIGTERM:           controller
SIGKILL:           owner
policy changes:    owner
```

Public output is convenient but may itself contain secrets. Make this clear in the generated default config.

---

## 8. Network security

### 8.1 Safe default binding

By default:

```text
127.0.0.1:8765
```

Also bind to IPv6 loopback if supported:

```text
[::1]:8765
```

Binding to `0.0.0.0` should require an explicit configuration change:

```toml
[server]
bind = "0.0.0.0"
allow_remote_bind = true
```

If the user enables a non-loopback address without TLS, print a strong warning or refuse unless another explicit unsafe override is enabled.

### 8.2 Recommended remote access

Prefer:

```text
ssh -L 8765:127.0.0.1:8765 workstation
```

or a private overlay network/reverse proxy that terminates TLS.

Do not implement custom cryptography.

### 8.3 HTTP framework

Use Axum on Tokio. Axum is designed around ergonomic, modular HTTP routing and works with Tokio and Hyper.

Suggested stack:

```text
tokio       async runtime
axum        HTTP routing and WebSockets
tower       middleware and service composition
tower-http  tracing, request limits, headers
serde       typed serialization
```

The exact dependency versions should be pinned in `Cargo.lock` and updated deliberately.

---

## 9. API design

Use a versioned API:

```text
/api/v1/
```

### 9.1 Read endpoints

```text
GET /api/v1/jobs
GET /api/v1/jobs/{job_id}
GET /api/v1/jobs/{job_id}/output
GET /api/v1/jobs/{job_id}/events
GET /api/v1/server
```

### 9.2 Control endpoints

```text
POST /api/v1/jobs/{job_id}/input
POST /api/v1/jobs/{job_id}/signals/interrupt
POST /api/v1/jobs/{job_id}/signals/terminate
POST /api/v1/jobs/{job_id}/signals/stop
POST /api/v1/jobs/{job_id}/signals/continue
POST /api/v1/jobs/{job_id}/signals/kill
DELETE /api/v1/jobs/{job_id}
```

Do not expose arbitrary numeric signals in the first release. Named, allow-listed operations are safer.

### 9.3 WebSocket

Use WebSockets for:

- live output events;
- state changes;
- optional terminal input;
- terminal resize requests.

Define typed messages.

Server to client:

```json
{
  "type": "output",
  "job_id": "01K1...",
  "sequence": 4812,
  "data_base64": "..."
}
```

```json
{
  "type": "state_changed",
  "job_id": "01K1...",
  "state": {
    "type": "exited",
    "code": 0
  }
}
```

Client to server:

```json
{
  "type": "send_input",
  "data_base64": "eWVzCg=="
}
```

Output must be treated as bytes, not assumed to be valid UTF-8. Terminal streams can contain arbitrary byte sequences.

### 9.4 API schema

Generate an OpenAPI description from typed request/response models or maintain one as a tested contract. An LLM agent benefits greatly from:

- explicit schemas;
- generated API clients;
- examples;
- stable error codes;
- machine-readable documentation.

Error format:

```json
{
  "error": {
    "code": "permission_denied",
    "message": "This token cannot send SIGTERM to this job.",
    "request_id": "01K1..."
  }
}
```

Do not expose Rust backtraces or internal filesystem details through HTTP errors.

---

## 10. Web interface

Keep the first interface deliberately small.

### Job list

Show:

- job name;
- state;
- start time;
- duration;
- profile;
- visibility;
- exit code;
- owner indicator.

### Job page

Show:

- live terminal output;
- connection state;
- process state;
- start time and duration;
- command, when authorized;
- working directory, when authorized;
- control buttons based on permissions.

**Controls:**

- Interrupt
- Terminate
- Pause
- Resume
- Kill
- Send input
- Download log

Require explicit confirmation for:

- terminate;
- kill;
- delete;
- restart, if later implemented.

Use a minimal frontend:

- server-rendered HTML;
- small JavaScript module for WebSockets;
- no Node build pipeline in the first version.

This makes Debian packaging and offline builds much easier. A React/Vite frontend would add dependency and packaging complexity without being necessary initially.

Static assets can be embedded into the Rust binary at compile time or installed under:

```text
/usr/share/jobwrap/web/
```

Embedding them reduces deployment mismatch, while installed assets make distribution customization easier. For the first version, embedding is simpler.

---

## 11. Persistence and logs

### 11.1 Database

Use SQLite for metadata:

```text
~/.local/state/jobwrap/jobwrap.db
```

Store:

- job metadata;
- state transitions;
- access profile snapshot;
- token hashes and scopes;
- browser session hashes;
- audit records;
- log metadata.

Do not store high-volume terminal output as database rows.

### 11.2 Output files

Store append-only output:

```text
~/.local/state/jobwrap/logs/{job_id}.terminal
```

Optionally store an index:

```text
{job_id}.index
```

The index can map output sequence numbers and timestamps to byte offsets.

Keep raw terminal bytes. A later version can optionally produce sanitized text.

### 11.3 Log limits

Enforce:

- maximum bytes per job;
- maximum total storage;
- retention days;
- completed-job cleanup;
- behavior on limit reached.

The default should keep the process running even when log recording reaches its limit:

```text
log recording disabled after maximum size
live terminal relay continues
```

Never terminate scientific work merely because the wrapper's log is full.

### 11.4 Crash recovery

After daemon restart:

- wrappers should reconnect automatically;
- jobs with connected wrappers become Running again;
- disconnected records should become Disconnected;
- process existence can be checked cautiously;
- PID existence alone must not be trusted because PIDs are reused.

Use a registration nonce and, where available, process start-time metadata to reduce PID-reuse confusion.

---

## 12. Project structure

Use a Cargo workspace:

```text
jobwrap/
├── Cargo.toml
├── Cargo.lock
├── rust-toolchain.toml
├── deny.toml
├── README.md
├── LICENSE
├── CHANGELOG.md
├── SECURITY.md
├── CONTRIBUTING.md
├── docs/
│   ├── architecture.md
│   ├── security-model.md
│   ├── api.md
│   ├── configuration.md
│   ├── terminal-behavior.md
│   └── debian-packaging.md
├── crates/
│   ├── jobwrap-cli/
│   ├── jobwrap-daemon/
│   ├── jobwrap-core/
│   ├── jobwrap-config/
│   ├── jobwrap-auth/
│   ├── jobwrap-protocol/
│   ├── jobwrap-pty/
│   ├── jobwrap-store/
│   └── jobwrap-web/
├── web/
│   ├── index.html
│   ├── job.html
│   ├── app.js
│   └── app.css
├── migrations/
├── tests/
│   ├── integration/
│   ├── fixtures/
│   └── scripts/
├── packaging/
│   ├── debian/
│   └── systemd/
└── .github/
    └── workflows/
```

### Crate responsibilities

**jobwrap-core**

Domain types and state machines:

- `JobId`
- `JobState`
- `Permission`
- `AccessPolicy`
- process identifiers
- events
- authorization decisions

It should have minimal dependencies and no HTTP or database code.

**jobwrap-config**

- TOML models;
- defaults;
- profile resolution;
- precedence;
- validation;
- source/provenance reporting;
- migrations between config versions.

**jobwrap-auth**

- password hashing;
- token creation and verification;
- session handling;
- authorization evaluation;
- rate limiting abstractions.

**jobwrap-protocol**

Typed messages between:

- wrapper and daemon;
- CLI and daemon;
- daemon and browser.

Use explicit protocol versions.

**jobwrap-pty**

Linux process and terminal logic:

- PTY allocation;
- process groups;
- signal forwarding;
- terminal mode guards;
- window-size propagation;
- relay loop.

Keep all low-level operating-system code here.

**jobwrap-store**

- SQLite repositories;
- migrations;
- transaction boundaries;
- log-file abstraction;
- retention cleanup.

**jobwrap-web**

- Axum routes;
- middleware;
- WebSockets;
- OpenAPI;
- static assets;
- HTTP response models.

**jobwrap-daemon**

Composition root:

- starts server;
- initializes storage;
- accepts wrapper registrations;
- maintains live job handles;
- performs recovery.

**jobwrap-cli**

- command-line parsing;
- wrapper mode;
- administrative subcommands;
- daemon auto-start;
- human-readable diagnostics.

---

## 13. Rust coding rules

For an LLM implementation agent, codify strict rules early.

### 13.1 Unsafe Rust

At the workspace level:

```rust
#![forbid(unsafe_code)]
```

Allow exceptions only in `jobwrap-pty` if a required system operation cannot be expressed through a safe library. Any exception should:

- be isolated to a small module;
- include a written safety invariant;
- have focused tests;
- be reviewed manually;
- use `#![deny(unsafe_op_in_unsafe_fn)]`.

Prefer established crates wrapping Linux APIs rather than writing raw FFI.

Rust's guarantees can be bypassed through `unsafe`, so minimizing and isolating it is important.

### 13.2 No unstructured dictionaries

Use named structs and enums for domain and configuration data. Do not use:

```rust
HashMap<String, serde_json::Value>
```

as the primary representation of configuration, jobs, policies, API requests, or protocol messages.

Maps are acceptable only where the problem is genuinely dynamic, such as HTTP headers or explicitly extensible metadata.

### 13.3 Error handling

Use:

- typed library errors with `thiserror`;
- contextual application errors with `anyhow` only at executable boundaries;
- stable machine-readable API error codes;
- error chains in CLI output;
- optional backtraces under a diagnostic environment setting.

Do not use:

- `unwrap()`
- `expect()`
- `panic!()`
- `todo!()`
- `unimplemented!()`

in production paths.

Allow `expect()` in tests or for compile-time/static invariants with an explanatory message.

### 13.4 Logging

Use `tracing` with structured fields:

- `job_id`
- `request_id`
- `principal_id`
- `operation`
- `permission`
- `pid`
- `process_group_id`

Never log:

- plaintext passwords;
- bearer tokens;
- session cookies;
- terminal input by default;
- complete request authorization headers.

### 13.5 Dependency policy

Require:

- committed `Cargo.lock`;
- minimal feature sets;
- no wildcard dependency versions;
- dependency license checks;
- vulnerability auditing;
- duplicate-dependency review;
- deliberate update PRs.

---

## 14. Tooling and quality gates

Rust provides first-party Cargo integrations for `rustfmt` and Clippy; Clippy is intended to catch common mistakes and questionable patterns.

The mandatory local/CI checks should be:

```text
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features
cargo doc --workspace --no-deps
cargo deny check
cargo audit
```

Additional useful tools:

```text
cargo-nextest       faster test runner
cargo-tarpaulin     coverage on Linux
cargo-mutants       mutation testing for critical logic
cargo-deny          licenses, bans, advisories, sources
cargo-audit         known vulnerability checks
cargo-semver-checks public API compatibility
cargo-llvm-cov      LLVM-based coverage
```

Do not require every optional tool for ordinary development, but run the important ones in CI.

### Compiler and lint configuration

Use strong workspace lint settings:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
missing_debug_implementations = "warn"
unreachable_pub = "warn"
unused_must_use = "deny"

[workspace.lints.clippy]
all = "deny"
pedantic = "warn"
nursery = "warn"
unwrap_used = "deny"
expect_used = "warn"
panic = "deny"
```

Apply exceptions narrowly and document them.

---

## 15. Testing strategy

### 15.1 Unit tests

Test pure logic extensively:

- configuration precedence;
- configuration validation;
- profile resolution;
- authorization decisions;
- token scope matching;
- job-state transitions;
- retention calculations;
- API serialization;
- signal-name conversion.

Authorization tests should be table-driven:

```text
anonymous + public output       → allow
anonymous + controller input    → deny
controller + SIGINT             → allow
controller + SIGKILL            → deny
owner + SIGKILL                 → allow
read-only token + output        → allow
read-only token + input         → deny
```

### 15.2 Property-based tests

Use property testing for:

- no lower role gains a higher permission;
- config serialization round trips;
- arbitrary malformed protocol messages do not panic;
- state-machine transitions preserve invariants;
- log indexes always refer to valid offsets;
- token parsers reject malformed input.

### 15.3 PTY integration tests

Run real commands under a PTY:

```text
printf
cat
bash -c 'read value; echo "$value"'
python3 -c 'import sys; print(sys.stdout.isatty())'
```

Verify:

- `isatty()` is true;
- colors can be emitted;
- input reaches the child;
- output reaches both wrapper and daemon;
- terminal resize propagates;
- Ctrl+C produces the correct result;
- process-group descendants receive signals;
- terminal mode is restored.

### 15.4 Process-tree tests

Use fixture programs that:

- spawn children;
- ignore SIGTERM;
- trap SIGINT;
- stop and continue;
- exit with known codes;
- close stdout unexpectedly;
- fork and keep running.

Verify that operations target the intended process group.

### 15.5 Authentication tests

Test:

- password setup and verification;
- wrong-password rate limiting;
- token expiration;
- token revocation;
- session expiration;
- cookie flags;
- CSRF rejection;
- endpoint permission coverage;
- audit event creation.

Every state-changing endpoint should have explicit tests for:

- anonymous
- authenticated browser
- read-only token
- controller token
- owner/local user

### 15.6 Fuzz testing

Fuzz:

- wrapper-daemon protocol decoding;
- WebSocket messages;
- configuration parsing;
- token parsing;
- terminal-output frame parsing;
- API input models.

The target condition is "never panic, never allocate without a configured bound, never bypass authorization."

### 15.7 End-to-end tests

Start a daemon in an isolated temporary XDG environment:

```text
XDG_RUNTIME_DIR=<temp>
XDG_CONFIG_HOME=<temp>
XDG_STATE_HOME=<temp>
```

Then:

1. launch a wrapped fixture process;
2. query its status;
3. stream output;
4. authenticate;
5. send input;
6. interrupt it;
7. verify exit state;
8. restart daemon;
9. verify persisted metadata.

### 15.8 Packaging tests

For every `.deb` build:

- install into a clean Ubuntu 20.04 container or VM;
- verify `/usr/bin/jobwrap`;
- verify daemon startup;
- run a wrapped command;
- test uninstall;
- test upgrade;
- ensure user state is preserved;
- ensure purge removes only system configuration, not user data unless explicitly intended.

PTY behavior should also be tested in a real VM because container terminal behavior can differ.

---

## 16. Debian package design

### 16.1 Installed files

The package should install:

```text
/usr/bin/jobwrap
/usr/libexec/jobwrap/jobwrapd
/usr/lib/systemd/user/jobwrapd.service
/usr/lib/systemd/user/jobwrapd.socket
/usr/share/doc/jobwrap/README.md.gz
/usr/share/doc/jobwrap/changelog.gz
/usr/share/man/man1/jobwrap.1.gz
/usr/share/man/man5/jobwrap.toml.5.gz
/usr/share/man/man8/jobwrapd.8.gz
/usr/share/bash-completion/completions/jobwrap
/usr/share/zsh/vendor-completions/_jobwrap
/usr/share/fish/vendor_completions.d/jobwrap.fish
```

Do not install a global writable state directory. All runtime and persistent state belongs to individual users.

### 16.2 Building the .deb

There are two useful tracks.

**Practical release package**

Use `cargo-deb` to create distributable binary `.deb` files from the Cargo project. `cargo-deb` is specifically intended to create Debian binary packages from Cargo projects.

This is suitable for:

- GitHub/GitLab releases;
- internal package repositories;
- direct `apt install ./jobwrap_...deb`;
- rapid iteration.

**Debian-policy package**

Maintain proper Debian packaging using:

- `debian/control`;
- `debian/rules`;
- `debian/changelog`;
- `debian/copyright`;
- `dh-cargo`/`debcargo` where appropriate.

Debian's Rust packaging policy identifies `debcargo` and `dh-cargo` as the tools automating much of Rust package construction.

This path is needed if the eventual goal is inclusion in Debian or Ubuntu repositories.

Initially, support `cargo-deb`, but structure the repository so proper Debian source packaging can be added without changing runtime layout.

### 16.3 Ubuntu 20.04 compatibility

Ubuntu 20.04 has an older glibc than newer build systems. Therefore:

- build the release binary on Ubuntu 20.04 or an environment with an equally old glibc;
- test it on a clean Ubuntu 20.04 image;
- avoid accidentally building on a newer distribution and assuming backward compatibility;
- consider a musl target only after PTY, NSS, TLS, and SQLite behavior have been validated.

For this application, building against Ubuntu 20.04's glibc is probably simpler and less surprising than forcing a completely static musl build.

### 16.4 Package scripts

Avoid root-level post-install behavior beyond normal package setup. Do not:

- start a system-wide daemon;
- create users;
- open firewall ports;
- create global secrets;
- enable remote access.

The per-user daemon should start only when that user invokes `jobwrap`, or through a user systemd unit.

---

## 17. CLI design

Use a consistent typed CLI, likely with `clap`.

```text
jobwrap [WRAPPER OPTIONS] -- COMMAND [ARGUMENTS...]
```

```text
jobwrap list
jobwrap show JOB
jobwrap logs JOB
jobwrap attach JOB
jobwrap signal JOB SIGNAL
jobwrap stop JOB
jobwrap open [JOB]
jobwrap daemon status
jobwrap daemon start
jobwrap daemon stop
jobwrap config init
jobwrap config validate
jobwrap config effective
jobwrap auth set-password
jobwrap token create
jobwrap token list
jobwrap token revoke TOKEN_ID
```

Support this convenient ambiguity:

```text
jobwrap python3 analysis.py
```

The parser can interpret an unrecognized first word as the executable. Reserve known subcommands. Also support explicit separation:

```text
jobwrap -- python3 analysis.py
```

The latter is useful when the command itself resembles a `jobwrap` subcommand.

### Exit behavior

When wrapping a command:

- normal exit: return the child exit code;
- signal exit: use conventional shell-compatible status where practical;
- wrapper internal failure before child start: dedicated documented codes;
- daemon failure during execution: keep the child running and report degraded monitoring;
- logging failure: keep the child running.

The wrapper should prioritize the user's process over its own monitoring features.

---

## 18. Error-reporting requirements

Errors should explain:

- what failed;
- why it matters;
- whether the child is still running;
- what the user can do;
- where diagnostics are stored.

Example:

```text
error: could not register the job with jobwrapd

The command has not been started.

Cause:
  permission denied while connecting to
  /run/user/1000/jobwrap/daemon.sock

The socket is owned by UID 1001, but the current UID is 1000.
For safety, jobwrap will not connect to a daemon owned by another user.

Try:
  jobwrap daemon status
  rm /run/user/1000/jobwrap/daemon.sock   # only after verifying no daemon is running
```

Degraded-mode example:

```text
warning: connection to jobwrapd was lost

The wrapped process is still running and remains attached to this terminal.
Web monitoring and remote control are temporarily unavailable.
jobwrap will continue attempting to reconnect.
```

Use distinct exit codes and stable API error identifiers.

---

## 19. Security threat model

Create `docs/security-model.md` before implementing authentication.

At minimum, cover:

### Assets

- ability to send input to processes;
- ability to signal or terminate processes;
- terminal output;
- command arguments;
- working directories;
- authentication credentials;
- agent tokens;
- audit history.

### Adversaries

- unauthenticated local-network user;
- authenticated read-only user;
- compromised low-scope LLM-agent token;
- malicious local process running under another UID;
- malicious website attempting CSRF/WebSocket hijacking;
- user cloning a repository containing hostile project configuration;
- attacker who can read log files;
- attacker attempting PID reuse or socket replacement.

### Trust boundaries

- browser ↔ HTTP server;
- CLI ↔ Unix socket;
- wrapper ↔ daemon;
- daemon ↔ SQLite/log files;
- wrapper ↔ child PTY;
- configuration file ↔ runtime policy.

### Required controls

- loopback binding by default;
- Unix socket permissions 0600;
- peer UID verification;
- scoped tokens;
- password rate limiting;
- CSRF protection;
- WebSocket origin checks;
- request body limits;
- output/backlog limits;
- no arbitrary signal numbers;
- no arbitrary remote process creation;
- no environment exposure;
- no shell interpolation of wrapped commands;
- audit records for control operations;
- strict project-config trust.

Most importantly, execute arguments directly:

```rust
Command::new(executable).args(arguments)
```

Never concatenate them into:

```text
/bin/sh -c "..."
```

unless the user explicitly invokes a shell themselves.

---

## 20. Implementation milestones

### Milestone 0: specification and invariants

Deliver:

- architecture document;
- security model;
- command-line specification;
- configuration schema;
- process state machine;
- API draft;
- explicit non-goals;
- acceptance tests written as scenarios.

No production code beyond workspace scaffolding.

### Milestone 1: local PTY wrapper

Implement:

```text
jobwrap -- COMMAND...
```

Requirements:

- PTY allocation;
- interactive input/output;
- terminal resize;
- process group;
- Ctrl+C;
- Ctrl+Z/resume;
- terminal restoration;
- correct exit status.

No daemon or web server yet.

This is the highest-risk systems component and should be validated first.

### Milestone 2: local daemon and typed protocol

Implement:

- daemon auto-start;
- private Unix socket;
- peer credential validation;
- job registration;
- state updates;
- output streaming;
- wrapper reconnection;
- `jobwrap list` and `jobwrap show`.

Still no TCP listener.

### Milestone 3: persistence

Implement:

- SQLite migrations;
- job metadata;
- append-only terminal logs;
- completed-job history;
- cleanup and retention;
- daemon restart recovery.

### Milestone 4: read-only web interface

Implement:

- loopback HTTP server;
- job list;
- job status;
- live output WebSocket;
- public/authenticated visibility;
- request limits;
- output backlog limits.

No remote control yet.

### Milestone 5: authentication

Implement:

- password setup;
- Argon2id verification;
- browser sessions;
- API tokens;
- token scopes;
- revocation;
- rate limiting;
- audit records.

### Milestone 6: authorized process control

Implement:

- input sending;
- SIGINT;
- SIGTERM;
- SIGSTOP;
- SIGCONT;
- SIGKILL;
- permission checks;
- confirmations in the UI;
- process-group targeting;
- action auditing.

### Milestone 7: configuration system

Implement:

- global user config;
- named profiles;
- CLI overrides;
- effective-config diagnostics;
- config validation;
- versioning;
- optional trusted project config.

Although configuration structures should be defined early, full profile functionality can arrive here after the process model is stable.

### Milestone 8: Debian package

Implement:

- `cargo-deb` metadata;
- installation paths;
- man pages;
- completions;
- systemd user units;
- clean Ubuntu 20.04 package tests;
- install/upgrade/remove/purge tests.

### Milestone 9: security hardening

Perform:

- endpoint permission audit;
- fuzzing;
- dependency audit;
- mutation testing of authorization rules;
- WebSocket abuse testing;
- log-exhaustion tests;
- PID-reuse tests;
- daemon/socket race tests;
- manual review of all unsafe code;
- external security review if remote access will be enabled.

### Milestone 10: release

Deliver:

- signed source tag;
- checksums;
- `.deb`;
- SBOM;
- changelog;
- migration notes;
- reproducible build instructions;
- security reporting policy.

---

## 21. Explicit version-1 non-goals

To keep the first implementation safe and achievable, exclude:

- execution of arbitrary new commands through the HTTP API;
- multi-user system daemon;
- root process management;
- container orchestration;
- full tmux/screen replacement;
- collaborative multi-writer terminal sessions;
- arbitrary Unix signal numbers;
- internet-facing TLS server;
- OAuth or enterprise identity providers;
- Windows and macOS support;
- automatic privilege escalation;
- environment-variable viewing;
- automatic shell-command parsing;
- plugins loaded into the daemon process.

The first release should do one thing well:

> Wrap a locally launched Linux process and securely expose monitoring and narrowly authorized control.

---

## 22. Instructions for an LLM coding agent

Give the implementation agent these operating constraints:

1. Work milestone by milestone; do not implement the web UI before PTY behavior passes integration tests.
2. Add or update tests with every behavioral change.
3. Use structs, enums, newtypes, and traits for domain concepts; avoid untyped dictionaries.
4. Do not add unsafe code outside the dedicated OS abstraction crate.
5. Do not add a dependency without documenting its purpose and evaluating maintenance, license, and security implications.
6. Never execute user arguments through an implicit shell.
7. Route all authorization decisions through one centralized service.
8. Preserve the child process whenever monitoring infrastructure fails.
9. Never weaken a security default merely to make a test pass.
10. Run formatting, Clippy, tests, and dependency checks before declaring a task complete.
11. Keep commits narrow and describe the invariant established by each change.
12. Record unresolved assumptions in `docs/decisions/` as architecture decision records.
13. Do not silently invent configuration behavior; update the schema and documentation first.
14. Do not expose internal Rust errors directly through the HTTP API.
15. Treat terminal data as arbitrary bytes.
16. Treat every PID as potentially stale or reused.
17. Verify Unix-socket ownership and permissions.
18. Include negative authorization tests for every protected operation.

A useful definition of done for each milestone is:

- implementation compiles without warnings;
- formatter passes;
- Clippy passes with warnings denied;
- unit and integration tests pass;
- security-relevant negative tests exist;
- user-visible behavior is documented;
- errors contain actionable context;
- no unfinished placeholders remain in production paths.

### Final recommendation

Use:

```text
Language:          Rust
Async runtime:     Tokio
HTTP/WebSockets:   Axum + Tower
CLI:               Clap
Serialization:     Serde + TOML
Persistence:       SQLite
Logging:           tracing
Password hashing:  Argon2id
Local IPC:         Unix-domain sockets
Process control:   Linux PTY + process groups
Packaging:         cargo-deb initially, proper Debian packaging afterward
Frontend:          server-rendered HTML + small plain JavaScript module
```

The first technical spike should focus entirely on this acceptance test:

```sh
jobwrap python3 -c '
import os
import signal
import sys
import time

print("stdin tty:", sys.stdin.isatty())
print("stdout tty:", sys.stdout.isatty())
print("pid:", os.getpid())
print("ready", flush=True)

while True:
    time.sleep(1)
'
```

It must behave indistinguishably from launching the same command directly in GNOME Terminal, including Ctrl+C, Ctrl+Z, resume, resize, output, and terminal restoration. Once that foundation is reliable, the daemon, web API, and authorization system can be built around it without compromising the original workflow.
