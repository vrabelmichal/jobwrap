# HTTP API and live monitoring

## Connection and authentication

Use the configured server URL, normally `http://127.0.0.1:8765`, with prefix
`/api/v1`. For agents, send `Authorization: Bearer TOKEN` using an existing
scoped token. Local HTTP is not local-owner authentication; the Unix socket CLI
has separate owner authority. A browser can instead log in with
`POST /api/v1/login`, JSON `{"password":"..."}`, and the returned
`jobwrap_session` cookie (`HttpOnly`, `SameSite=Strict`); logout is
`POST /api/v1/logout`.

In these examples `JOBWRAP_TOKEN` is supplied securely by the caller, and
`JOBWRAP_BASE_URL` and `JOBWRAP_JOB_ID` contain the actual address and ID. Avoid
shell tracing or logging credentials. An environment variable here is an
example client convention, not a jobwrap configuration override.

```sh
curl --fail-with-body --max-time 15 \
  -H "Authorization: Bearer $JOBWRAP_TOKEN" \
  "$JOBWRAP_BASE_URL/api/v1/jobs/$JOBWRAP_JOB_ID"
```

## Read endpoints

| GET path (relative to `/api/v1`) | Response/use |
| --- | --- |
| `/server` | Version, `auth_required`, bind, port |
| `/auth` | `authenticated`, `password_set` |
| `/jobs` | Array of visible summaries; uses `display_name`, `profile_name`, `state`, and log counters |
| `/jobs/{id}` | Status object: `id`, `name`, `profile`, `state`, timestamps, `terminal_attached` |
| `/jobs/{id}/details` | Permission-filtered command/cwd, process snapshot, log metadata, and permission flags |
| `/jobs/{id}/output?from=N` | Base64 terminal log slice, at most 1 MiB, starting at byte offset N |
| `/jobs/{id}/events` | State/event history (`id`, `at`, `kind`) |

List and single-job payloads have different field names. State is a tagged
object, for example `{"type":"running"}` or `{"type":"exited","code":0}`.
Other types are `registering`, `stopped`, `signaled` (with `signal`),
`disconnected`, and `lost`. Only `exited` with code 0 establishes a successful
process exit; verify the workload's intended result separately. Treat
`disconnected`/`lost` as uncertainty and inspect the launch session before retrying.
`/details` process values can be null, and its permission flags describe the
current caller rather than granting access.

## Incremental recorded output

`GET /jobs/{id}/output?from=0` returns:

```json
{"job_id":"...","sequence_start":0,"truncated":false,"data_base64":"..."}
```

Despite the field name `sequence_start`, this log retrieval path uses a **byte
offset**. Decode `data_base64` as bytes and set the next offset to
`sequence_start + len(decoded_bytes)`, not the encoded length or character count.
Fetch subsequent slices until empty; for a running job, wait before polling
again. Persist the cursor to avoid rereading large logs. An empty slice means no
recorded bytes are available there now, not that the process finished. Check
state separately. After exit, drain the remaining available recorded output.

Terminal data need not be UTF-8. Preserve raw bytes where necessary and use an
incremental decoder if presenting text across chunk boundaries. Recording may
stop at log limits or be disabled. Inspect `/details` → `log.truncated` or list
→ `log_truncated`; do not assume the slice's `truncated` field proves the entire
log is complete.

## WebSocket

Connect to `/api/v1/jobs/{id}/ws` using `ws://` or `wss://` as appropriate and
the same bearer header or browser session cookie. Output permission is required.
The daemon allows at most 128 concurrent live sockets.

Server messages:

```json
{"type":"output","job_id":"...","sequence":4812,"data_base64":"..."}
{"type":"state_changed","job_id":"...","state":{"type":"exited","code":0}}
```

The live `sequence` is an output-event sequence, **not** the recorded-log byte
cursor. The stream supplies live events, without historical replay or a
guaranteed lossless handoff to log retrieval. Client text control messages are
ignored; send input/signals over HTTP. On disconnect, reconnect with backoff,
recheck current status, and recover available recorded bytes using the saved
byte cursor. Do not infer completion from a closed socket or rely on receiving
every state event.

## Control endpoints

Use controls only within the user's task authority; read-only monitoring does
not authorize cancelling a calculation or sending answers to an upload prompt.

| Method/path (relative to `/api/v1`) | Body/action |
| --- | --- |
| `POST /jobs/{id}/input` | `{"data_base64":"..."}`; base64-encoded input bytes |
| `POST /jobs/{id}/signals/interrupt` | SIGINT, no body |
| `POST /jobs/{id}/signals/terminate` | SIGTERM, no body |
| `POST /jobs/{id}/signals/stop` | SIGSTOP, suspension |
| `POST /jobs/{id}/signals/continue` | SIGCONT, resume |
| `POST /jobs/{id}/signals/kill` | SIGKILL, forced termination; owner-only by default |
| `DELETE /jobs/{id}` | Remove a finished record and log; owner-only by default |

Successful controls return HTTP 204. Signals target the child's process group.
Poll state to confirm their result. For newline-terminated input, encode the
newline too (for example `y\n` is `eQo=`). The HTTP request-body cap is 256 KiB,
including JSON and base64 overhead; split input into suitably small requests.

```sh
curl --fail-with-body --max-time 15 -X POST \
  -H "Authorization: Bearer $JOBWRAP_TOKEN" \
  "$JOBWRAP_BASE_URL/api/v1/jobs/$JOBWRAP_JOB_ID/signals/interrupt"
```

Matching token scopes use `signal:int`, `signal:term`, `signal:stop`,
`signal:cont`, and `signal:kill`, rather than the full endpoint names.
`jobs:list` does not by itself grant per-job status/output/control.
Requests carrying an `Origin` header must pass the server's same-origin check;
ordinary non-browser clients can omit that header.

Errors use `{"error":{"code":"permission_denied","message":"..."}}`.
Codes include `bad_request`, `unauthorized`, `permission_denied`, `not_found`,
`conflict`, `rate_limited`, and `internal_error`. Inspect the HTTP status and
error body. Back off on transient read failures; do not automatically replay
input, deletion, or other mutations after an ambiguous response. Permission
failure calls for the appropriate existing authority, not disabling protections.

## Experimental launch and inspection

Prefer local `jobwrap wrap` for starting workloads. `POST /api/v1/launch` is
disabled by default. Even when `[launch] enabled = true`, the default
`require_preview = true` fails closed because launch previews are not implemented.
Only an owner-explicitly configured `require_preview = false` allows the
new-terminal path (`gnome-terminal` or `xterm`); managed and existing-terminal
launch modes fail closed. HTTP launch requires an explicit `idempotency_key`:
preserve it when retrying the same intended launch. Acceptance indicates pending
terminal startup, not a running or successful workload. Do not change these
settings as part of routine monitoring.

Static target inspection is available through
`POST /documentation/identify` and man-page search/fetch endpoints. Executable
help probes are disabled by default and are not an OS sandbox. Avoid executing
probes merely to discover how to monitor an existing job.
