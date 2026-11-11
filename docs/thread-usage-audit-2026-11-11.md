# Jobwrap usage audit from T3 Code history

Reviewed 2026-11-11 against source commit `d0b4359` (0.3.0).

## Coverage and confidence

On vrabel-desktop, used T3 MCP project/thread listing, per-project message
search, paged message reads, and a focused activity read. Search is explicitly
non-exhaustive and skips subagents; this is an evidence-based backlog, not a
claim that every invocation was inspected. Read the principal incident and
feature threads plus selected scientific campaign and deployment threads.

Cross-machine MCP requests fail because linked environments are disabled.
After SSH was authorized, queried the 7950's T3 SQLite message projections
read-only (`mode=ro`), including archived/imported messages. The v2 projection
contained 84 messages mentioning jobwrap across four threads; the older
projection contained 55 matching messages, with overlapping imported history.
Inspected incident-related excerpts in both. Did not read credentials or change
services, workloads, settings, or packages. Remote thread links were not returned
by MCP; remote evidence below uses database thread IDs and message timestamps.

The 7950 CLI currently reports **0.2.1**, clean build, commit `884ff2e`.
That does not establish the daemon's or historical workers' versions. Most
November threads do not record exact build identities; release dates alone
cannot establish which binary ran a job.

## Evidence register

**E1 — 7950 wrapper loss, October 8.** Job `01M4D55Q3W2NB4B5J5K73RP6SP`
ran under 0.1.1 through a persistent Codex execution session. Wrapper vanished;
child and daemon survived. Recording stopped at 36,398 bytes, without truncation
or a disconnection event. Initially the scientific log kept advancing. Later,
the originating thread reported its log writer blocked on the disconnected
terminal, requiring checkpoint recovery. The original termination cause remains
unknown. The subsequent 0.1.3 run was manually placed under a user service,
before native detached mode existed. Remote thread
`ba469ef6-6474-4749-bb21-1d3547535b58`, October 8, especially 12:04:47 UTC.
The investigator's copy is also in
<t3-thread://v2/77d55150-135c-40c2-b525-8a5a728d6359/861d6eee-d2cf-4815-8ad2-29e535c98eea>.

**E2 — desktop preview server disappeared, October 21.** Job
`01M5CCK4GX46ED5YT1VK0XS0H5` was disconnected, both recorded PIDs absent,
and port 18881 down. Cause was not established. The investigation accidentally
ran an executable named `status` because the old CLI treated unknown commands
as workloads; that extra job exited 127. This directly motivated the 0.2.0
explicit CLI and native detached mode. Exact incident binary version unknown;
behavior predates 0.2.0.
<t3-thread://v2/77d55150-135c-40c2-b525-8a5a728d6359/33a69a00-39d8-4c89-9b5a-96f0bcc10425>

**E3 — 7950 detached processing survived monitoring loss, November 5.**
Originating thread explicitly says `jobwrap run --detach` was used. Several
workers logged daemon connection closure when the T3/Codex scope stopped.
The investigator inferred an app-server restart killed the monitoring daemon;
the workload's independent service survived. The scientific dashboard had
incorrectly treated disconnection as stopped. It was patched to inspect the
campaign process inside its systemd service and otherwise report monitoring
unavailable. Remote thread `ba469ef6-6474-4749-bb21-1d3547535b58`,
22:35:49 and 22:37:02 UTC. Exact historical version unrecorded; current remote
CLI is 0.2.1. This is independent evidence for daemon lifetime and reconnection
work, rather than another foreground-launch mistake.

**E4 — detached controller no longer controllable, November 6.**
Job `01M6R52PFF2WKQ3DH3NVPA6XGK` showed disconnected while the controller
still ran. Activity positions 242 and 247 show the live process and
`jobwrap stop` returning “the job is no longer controllable.” The agent used a
direct SIGINT. Same activity shows a Rust stdout broken-pipe panic after
`jobwrap list | head -5`. Earlier, a background wait produced no output and
the agent suspected PATH, switching to an absolute executable path. That PATH
diagnosis was a hypothesis, not a proven jobwrap fault. Exact build unknown.
<t3-thread://v2/77d55150-135c-40c2-b525-8a5a728d6359/657fff7d-5a5a-4193-aef8-c1948b73973b>

**E5 — selective collector replacement, November 11.** Agent's process-name
lookup repeatedly matched its own shell; it switched to jobwrap's recorded PID.
It wanted to restart the collector without killing an ongoing copy, but noticed
stopping its systemd service would kill that connection too. This exposes a
supervision-boundary problem, not proof of a signal implementation bug.
<t3-thread://v2/77d55150-135c-40c2-b525-8a5a728d6359/34cd764d-77cb-40b4-a5ba-76896c8d4310>

**E6 — reboot recovery and polling.** Remote thread
`201be671-9800-41d9-9923-9a59ab49594a`, October 26 16:55:45 UTC, reports
reboot stopped daemon and detached services; agent restarted all three review
queues while preserving saved revisions. Remote thread
`f3619567-f36f-4829-86fb-059d9a74275b`, November 6 13:52:05 UTC, contains
an ad hoc loop parsing `jobwrap show` with awk every 30 seconds until state
differs from running. Such a loop cannot safely equate disconnection with exit.

**E7 — private shared links denied, 0.1.1.** A finished 7950 job remained in
the database, but opening its shared URL produced an authorization error.
Investigation found browser-session handling, not completion/retention, was
responsible. SameSite=Lax and login-return navigation shipped in 0.1.2; host
identification shipped in 0.1.3. See investigator thread under E1.

**E8 — information/history improvements already requested.** Paging, hiding
old finished jobs, date filters, detached service information and opt-in pruning
were implemented for 0.3.0.
<t3-thread://v2/77d55150-135c-40c2-b525-8a5a728d6359/800c2bbf-482c-4c15-a661-3ff3b9533a17>

**E9 — application failures must remain distinct.** Campaign job
`01M61WG5V41P7XZWPZBE0BY8NY` exited 2 on a storage reserve guard, with
102.98 GiB available against 103 GiB required. Requeueing needed edits to the
campaign ledger and attempt directories. Those are application recovery issues,
not established jobwrap failures. Similarly E4's fast training failures were
remote storage preflight failures.
<t3-thread://v2/77d55150-135c-40c2-b525-8a5a728d6359/dff302db-e0bf-45c6-9a73-63259e0210d8>

## Recommended backlog

### P0: independent daemon lifetime

E3 shows detached workers are insufficient if auto-started jobwrapd remains in
an agent cgroup. Use the packaged user service for normal daemon startup where
available; make the active daemon's supervisor and lifetime visible. Keep
foreground development mode explicit. This is an integration/startup issue:
a packaged service already exists, so merely adding another service file is
not the solution.

Acceptance: terminating the launching T3 scope leaves daemon, workers,
recording and control functional. Status explains whether the daemon is managed
by systemd or tied to the launching session.

### P0: reconnect workers and preserve output and final results

E1, E3 and E4 show the central unresolved problem. Current terminal-behavior
documentation explicitly says daemon restart loses recording/control and there
is no wrapper reconnection/adoption. 0.3.0 adds service queries, not restoration
of the wrapper protocol. Registry reconciliation marks ended detached services
`lost`, with unknown exit status and a finish timestamp meaning detection time.

Implement authenticated worker reconnection with persistent job identity,
bounded local output spooling, offset-based replay, and a durable final result.
Distinguish reconnection of a surviving wrapper from adoption of a workload
whose PTY-owning wrapper is already gone; the latter cannot restore the old PTY.

Acceptance: restart daemon during output; later logs have no gaps or duplicates,
control returns, and an exit while the daemon is absent is eventually recorded
with the real outcome. Preserve boot/process identity against PID reuse.

### P1: consistent status, diagnosis and safe recovery

Expose workload liveness, wrapper connectivity, recording freshness and control
availability separately in CLI/API/UI. Include last contact and its observed
reason, service identity and available evidence. Provide one diagnostic command
with explicit recovery guidance. Offer a service-based stop for a verified
detached unit when wrapper control is unavailable, with clear scope.

E3's application-specific systemd fallback should become unnecessary. 0.3.0
partly addresses liveness; it does not recover control or establish success.
Acceptance: disconnected plus live service never implies completion; stale PIDs
are never signaled; observed connection EOF is not described as a proven kill
cause. Keep actual finish time separate from detection time.

### P1: machine-readable CLI and a reliable wait operation

Add structured launch/show/list/event output and `wait JOB_ID` with timeout and
explicit unknown/disconnected semantics. E6's polling and E4's shell/PATH
workaround show the need. Launch should report registration separately from
readiness and completion. Current CLI command definitions have no JSON or wait
interface; the HTTP API already offers structured data.

Acceptance: agents obtain ID, URL and unit without scraping columns; waiting
does not report success on disconnection and survives transient daemon absence
according to a documented policy. Document absolute executable paths for
noninteractive shells and do not silently ignore failed status queries.

### P1: handle broken stdout pipes without panic

E4 contains a concrete panic from a normal `list | head` workflow. CLI output
writers should treat BrokenPipe deliberately, without a Rust panic/backtrace.
Acceptance: list/log output piped to an early-closing consumer behaves quietly
and predictably, without altering workload state. Historical build unknown;
reproduce on current source before selecting the patch.

### P1: preserve versions and provenance per job

Persist launching CLI/worker and daemon version, source commit and dirty flag,
plus host/boot identity and launch mode. Display current daemon identity
separately from historical identities. Existing build metadata dates to 0.1.2,
but this audit still cannot attribute most later incidents to exact builds.
Surface CLI/daemon mismatches and provide an upgrade/restart diagnostic.

### P2: deliberate reboot recovery for servers

E6 required manually restoring three queues. Native detach promises session
survival, not reboot survival; that is documented behavior. Add opt-in named
persistent server definitions with explicit restart policy and readiness check,
while keeping one-off computation restart disabled by default. Record saved
state locations and explain user-manager/logout/lingering requirements.
Acceptance: opted-in servers return after reboot with their existing review
state; arbitrary scientific computations are never automatically duplicated.

### P2: explicit supervision boundaries and workload context

E5 needs clearer process-tree/cgroup inspection and a preview of stop scope.
Recommend independent jobs for transfers that must outlive a collector restart;
do not add ambiguous arbitrary-PID killing. Optional project/thread labels,
workload log paths, readiness URLs and progress/stop-reason annotations would
help connect process status to application progress. E9 shows why jobwrap
should expose application-reported reasons without owning campaign recovery.

## Already addressed; verify deployment rather than reimplement

- 0.1.2: shared-link cookies, login return flow, consistent build identity (E7).
- 0.1.3: terminal-write failure handling, signal-output draining, disconnection
  events/reasons and clearer unknown outcome text (E1). Does not protect a
  killed wrapper or reconstruct its terminal.
- 0.2.0: native independent detached jobs, explicit `run`, read-only `status`
  alias (E2). Manual user-service wrapping from E1 should no longer be necessary.
- 0.2.1: broader agent skill packaging. Verify actual discovery per agent;
  packaging alone does not prove an agent read current instructions.
- 0.3.0: service inspection/reconciliation, paging/date filters/history hiding,
  opt-in pruning (E8). The 7950 currently has an older CLI; upgrading addresses
  those gaps but does not implement reconnection. Do not restart active services
  merely to apply an upgrade without accounting for monitoring loss.

Suggested implementation sequence: independent daemon startup, durable worker
reconnection/results, unified diagnosis and safe control, then structured CLI
and wait. The broken-pipe fix is a small separate task. Treat reboot recovery
and richer application metadata as optional subsequent features.
