# API

All HTTP endpoints are under `/api/v1`. Errors use a stable JSON shape:

```json
{
  "error": {
    "code": "permission_denied",
    "message": "This token cannot send SIGTERM to this job."
  }
}
```

Stable error codes: `bad_request`, `unauthorized`, `permission_denied`,
`not_found`, `conflict`, `rate_limited`, `internal_error`.

## Authentication

* **Cookie session** — after `POST /api/v1/login`, the `jobwrap_session`
  cookie is `HttpOnly`, `SameSite=Lax`, allowing authenticated navigation from
  links shared by other apps. State-changing browser requests remain subject
  to the same-origin check. An unauthenticated private job page offers a login
  link that returns to the job after login. `/login?return_to=/jobs/JOB_ID`
  preserves that destination; unsupported destinations fall back to `/`.
* **Bearer token** — `Authorization: Bearer <token>` for agent use.
* **Local owner** — the CLI talks over the private Unix socket and is treated
  as the owner.

## Read endpoints

`GET /api/v1/server` reports `version`, the full `git_commit`, and
`build_dirty`, alongside the server's authentication and bind settings.
The version matches the application's GitHub release (`vVERSION`) and the
upstream part of its Debian package version (`VERSION-REVISION`).

| Method | Path                          | Notes                          |
|--------|-------------------------------|--------------------------------|
| GET    | `/api/v1/server`              | version, source commit, build state, auth required, bind |
| GET    | `/api/v1/auth`                | current session state          |
| GET    | `/api/v1/jobs`                | job summaries visible to you   |
| GET    | `/api/v1/jobs/{job_id}`       | non-sensitive status detail    |
| GET    | `/api/v1/jobs/{job_id}/output?from=N`| up to 1 MiB from byte offset N (`data_base64`) |
| GET    | `/api/v1/jobs/{job_id}/events`| state transition history       |

## Control endpoints

| Method | Path                                    | Notes             |
|--------|-----------------------------------------|-------------------|
| POST   | `/api/v1/jobs/{job_id}/input`           | body `{data_base64}` |
| POST   | `/api/v1/jobs/{job_id}/signals/interrupt` | SIGINT          |
| POST   | `/api/v1/jobs/{job_id}/signals/terminate` | SIGTERM         |
| POST   | `/api/v1/jobs/{job_id}/signals/stop`    | SIGSTOP           |
| POST   | `/api/v1/jobs/{job_id}/signals/continue`| SIGCONT           |
| POST   | `/api/v1/jobs/{job_id}/signals/kill`    | SIGKILL           |
| DELETE | `/api/v1/jobs/{job_id}`                 | delete a finished record and log |

Only named signals are exposed; arbitrary numeric signals are not.

## WebSocket

`GET /api/v1/jobs/{job_id}/ws` streams live output and state changes after the
connection is authorized.

Live output requires output permission, not merely status permission. At most
128 live WebSockets are accepted by one daemon.

## Experimental launch and documentation endpoints

`POST /api/v1/launch` requires an explicit `idempotency_key`. Process creation
is disabled by default. With `[launch] enabled = true`, the default
`require_preview = true` still fails closed because launch previews are not yet
implemented. If an owner explicitly sets `require_preview = false`, only
`new_terminal` mode is available; managed and existing-terminal modes return
an error without starting a process.

The web interface exposes the same launch path to logged-in users through a
form page at `/jobs/new` (linked from the jobs list). When creation is
disabled or fails closed, the page shows the responsible configuration
instead of the form.

Static inspection is available through
`POST /api/v1/documentation/identify` and the man-page endpoints. Identification
reads at most 8 KiB of a regular target. Man commands have output/time limits.
Help-probe preview/execution endpoints exist but probes are disabled by default
and are not an OS security sandbox.

Server -> client:

```json
{ "type": "output", "job_id": "01K1...", "sequence": 4812, "data_base64": "..." }
{ "type": "state_changed", "job_id": "01K1...", "state": { "type": "exited", "code": 0 } }
```

Terminal data is bytes, not UTF-8; it is always transported as base64.

## CLI over the Unix socket

The same operations are available to the local user through `jobwrap`:

```text
jobwrap list
jobwrap show JOB_ID
jobwrap logs JOB_ID
jobwrap signal JOB_ID INT
jobwrap stop JOB_ID
jobwrap auth set-password
jobwrap token create --name agent --scope jobs:list --scope job:*:output
```

## Token scopes

Tokens carry scopes such as:

```text
jobs:list
job:*:status
job:*:output
job:<job_id>:signal:int
```

Scopes are parsed into a structured grant at creation; authorization never
matches raw scope strings.
