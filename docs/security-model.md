# Security model

This document describes the assets, adversaries, trust boundaries, and
required controls for jobwrap. It is normative: every design decision that
relaxes or strengthens a control listed here must be reviewed against this
document.

## Assets

* ability to send input to processes;
* ability to signal or terminate processes;
* terminal output;
* command arguments;
* working directories;
* authentication credentials;
* agent tokens;
* audit history.

## Adversaries

* unauthenticated local-network user;
* authenticated read-only user;
* compromised low-scope LLM-agent token;
* malicious local process running under another UID;
* malicious website attempting CSRF/WebSocket hijacking;
* user cloning a repository containing hostile project configuration;
* attacker who can read log files;
* attacker attempting PID reuse or socket replacement.

## Trust boundaries

* browser <-> HTTP server;
* CLI <-> Unix socket;
* wrapper <-> daemon;
* daemon <-> SQLite/log files;
* wrapper <-> child PTY;
* configuration file <-> runtime policy.

## Required controls

### Network

* the HTTP server binds to loopback (`127.0.0.1` and `[::1]`) by default;
* the daemon refuses non-loopback binds, including when the legacy
  `allow_remote_bind` field is true;
* the single exception is the explicit `bind = "tailscale"` target, which
  listens only on the `tailscale0` VPN interface. That interface is
  authenticated (WireGuard) and private to the tailnet, so binding it does
  not expose the daemon to the LAN or the internet;
* remote access is intended through the tailscale bind, `ssh -L`, or a
  TLS-terminating reverse proxy, not through a plaintext internet listener;
* no custom cryptography is implemented.

### Unix socket trust

* the socket file mode is `0600`;
* the daemon verifies the peer UID via `SO_PEERCRED` equals the daemon UID;
* callers never supply their own UID;
* local owner operations through the socket require no password.

### Authorization

* authentication and authorization are separate concepts;
* every HTTP handler, WebSocket action, and local command routes through the
  single `jobwrap_core::authorize` function;
* authorization is expressed as typed `Permission` + `AccessLevel`, never as
  ad-hoc strings;
* token scopes are evaluated independently: a token may perform an action only
  if it holds the matching grant for the job;
* `SIGKILL`, restart, delete, and policy changes default to `owner`;
* public output is public by default; the generated config warns that public
  output may itself contain secrets.

### Secrets

* passwords are entered through a non-echoing terminal prompt;
* passwords are hashed with Argon2id and a random salt;
* passwords are never passed as CLI arguments, logged, stored in the
  environment, or returned by the API;
* API tokens are random 32-byte values; only their SHA-256 hash is stored;
* tokens are displayed exactly once at creation;
* browser sessions are opaque random values; only their hash is stored, in an
  `HttpOnly` + `SameSite=Lax` cookie. Lax allows authenticated navigation to
  job links shared by agents or other apps; state-changing requests still use
  the same-origin check below.

### Rate limiting and abuse

* password attempts are rate limited;
* request bodies are size limited;
* WebSocket connections are authorized before upgrade;
* state-changing HTTP requests require an exact same-origin match when an
  Origin header is present;
* output is bounded per job (`maximum_log_bytes`) and recording stops rather
  than terminating the process when the limit is reached;
* aggregate logs are capped at 4 GiB; daemon socket connections, live
  WebSockets, wrapper output queues, sessions, probe records, and probe
  subprocesses have fixed bounds.

### Process safety

* daemon/API process creation is disabled by default;
* managed and existing-terminal daemon launches fail closed; only the opt-in
  new-terminal helper path is implemented;
* wrapped commands are executed directly
  (`Command::new(executable).args(arguments)`), never via an implicit shell;
* no arbitrary numeric signals are exposed — only named allow-listed signals;
* environment variables are never exposed through the API;
* every PID is treated as potentially stale or reused.

Help probes are also disabled by default. When explicitly enabled they use a
private working directory, filtered environment, closed stdin, output limits,
a timeout, and a dedicated process group. This is not filesystem or syscall
containment: a probed program runs with the user's Unix permissions.

## Default access recommendations

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

## Incident response

* control operations are recorded in the audit log with the acting principal;
* state-changing endpoint tests cover anonymous, authenticated browser,
  read-only token, controller token, and owner principals.
