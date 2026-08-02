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
  cookie is `HttpOnly`, `SameSite=Strict`.
* **Bearer token** — `Authorization: Bearer <token>` for agent use.
* **Local owner** — the CLI talks over the private Unix socket and is treated
  as the owner.

## Read endpoints

| Method | Path                          | Notes                          |
|--------|-------------------------------|--------------------------------|
| GET    | `/api/v1/server`              | version, auth required, bind   |
| GET    | `/api/v1/auth`                | current session state          |
| GET    | `/api/v1/jobs`                | job summaries visible to you   |
| GET    | `/api/v1/jobs/{job_id}`       | full record                    |
| GET    | `/api/v1/jobs/{job_id}/output`| terminal bytes (`data_base64`) |
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
| DELETE | `/api/v1/jobs/{job_id}`                 | delete record     |

Only named signals are exposed; arbitrary numeric signals are not.

## WebSocket

`GET /api/v1/jobs/{job_id}/ws` streams live output and state changes after the
connection is authorized.

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
