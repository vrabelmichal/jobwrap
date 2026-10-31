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

`GET /api/v1/server` reports `host_name`, `version`, the full `git_commit`,
and `build_dirty`, alongside the server's authentication and bind settings.
The version matches the application's GitHub release (`vVERSION`) and the
upstream part of its Debian package version (`VERSION-REVISION`).

| Method | Path                          | Notes                          |
|--------|-------------------------------|--------------------------------|
| GET    | `/api/v1/server`              | version, source commit, build state, auth required, bind |
| GET    | `/api/v1/auth`                | current session state          |
| GET    | `/api/v1/jobs`                | job summaries visible to you; optional filters below |
| GET    | `/api/v1/jobs/query`          | one filtered page with totals and per-state counts |
| GET    | `/api/v1/jobs/{job_id}`       | non-sensitive status detail    |
| GET    | `/api/v1/jobs/{job_id}/details` | permission-filtered command/cwd, process snapshot, detached service |
| GET    | `/api/v1/jobs/{job_id}/output?from=N`| up to 1 MiB from byte offset N (`data_base64`) |
| GET    | `/api/v1/jobs/{job_id}/events`| state transition history       |

### Filtering and paging job lists

Both list endpoints accept these query parameters:

| Parameter | Meaning |
|-----------|---------|
| `state` | `all` (default), `active` (registering/running/stopped), `attention` (disconnected/lost), `finished` (exited/signaled) |
| `q` | case-insensitive text over name, profile, ID, PIDs, state, and service unit |
| `started_after`, `started_before` | start-time range |
| `finished_after`, `finished_before` | finish-time range; excludes unfinished jobs |
| `hide_finished_older_than_days` | hide finished jobs that ended more than N days ago; unfinished jobs are kept |
| `offset`, `limit` | page window, newest first; `limit` is 1–1000 |

Times are RFC 3339 or `YYYY-MM-DD` (a UTC day). Lower bounds are inclusive and
upper bounds exclusive, except that a plain date as an upper bound includes
that whole day. A `lost` record without a finish time is treated as finishing
at its start. Invalid values return `bad_request`.

`GET /api/v1/jobs` still returns a plain array and applies no limit unless one
is given. `GET /api/v1/jobs/query` defaults to `limit=50` and returns:

```json
{
  "jobs": [ ... ],
  "total": 132,
  "offset": 0,
  "limit": 50,
  "counts": { "all": 132, "active": 2, "attention": 1, "finished": 129 },
  "hidden_finished": 418
}
```

`total` counts every match before paging. `counts` applies the time and text
filters but not `state`. `hidden_finished` counts jobs excluded only by
`hide_finished_older_than_days`. The dashboard uses
`hide_finished_older_than_days=7` unless a date range is chosen or older jobs
are requested.

### Detached service state

For a job started with `jobwrap run -d`, summaries and details include
`service_unit` (`jobwrap-job-JOB_ID.service`). While the job is not finished,
`/details` also asks the systemd user manager and returns `service`:

```json
{
  "unit": "jobwrap-job-01K1....service",
  "available": true,
  "load_state": "loaded",
  "active_state": "active",
  "sub_state": "running",
  "main_pid": 41234,
  "active_since": "2026-10-31T09:12:44Z",
  "memory_bytes": 52428800,
  "cpu_usage_nanoseconds": 1250000000,
  "tasks": 3
}
```

`available: false` (with `error`) means the manager could not be asked and
says nothing about the job. Usage counters are `null` when the manager has no
accounting for them (for example on cgroup v1). For a `disconnected` detached
job, a running service means the workload is still alive; the process
snapshot is then shown only when the service's main PID is the recorded
worker and the child is still its child.

## Control endpoints

| Method | Path                                    | Notes             |
|--------|-----------------------------------------|-------------------|
| POST   | `/api/v1/jobs/{job_id}/input`           | body `{data_base64}` |
| POST   | `/api/v1/jobs/{job_id}/signals/interrupt` | SIGINT          |
| POST   | `/api/v1/jobs/{job_id}/signals/terminate` | SIGTERM         |
| POST   | `/api/v1/jobs/{job_id}/signals/stop`    | SIGSTOP           |
| POST   | `/api/v1/jobs/{job_id}/signals/continue`| SIGCONT           |
| POST   | `/api/v1/jobs/{job_id}/signals/kill`    | SIGKILL           |
| DELETE | `/api/v1/jobs/{job_id}`                 | delete a finished record, its events, and log |
| POST   | `/api/v1/jobs/prune`                    | body `{older_than_days, dry_run}`; see below |

Only named signals are exposed; arbitrary numeric signals are not.

`POST /api/v1/jobs/prune` deletes finished jobs (exited, signaled, lost) that
ended more than `older_than_days` (at least 1) days ago, with their logs and
events; `"dry_run": true` only lists them. It returns
`{jobs, dry_run, freed_log_bytes}`. It requires `[retention] allow_web_prune =
true` and a browser login session; API tokens and anonymous requests get
`permission_denied`. Cross-origin browser requests are rejected.

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

`jobwrap run -- COMMAND` starts a foreground job. Use
`jobwrap run -d -- COMMAND` for a detached local job; this owner-only socket
operation does not enable or use the experimental HTTP launch API. It requires
a working systemd user manager. The CLI returns after job registration, and
`show`/`status` report the eventual exit code or termination signal.

The same operations are available to the local user through `jobwrap`:

```text
jobwrap list
jobwrap show JOB_ID            # includes detached service state
jobwrap prune --older-than 90 --dry-run
jobwrap status JOB_ID
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
