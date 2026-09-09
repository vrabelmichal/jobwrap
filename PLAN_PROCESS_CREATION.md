# Plan: Secure Process Creation, Terminal Launching, and Documentation Discovery

> **Implementation-aware revision** — 2026-08-03.
>
> This plan was originally written without knowledge of the actual codebase.
> Sections marked with `[IMPL: ...]` reference the specific crate, module, and
> approach used in the current Rust implementation.
>
> **Corrected implementation status (2026-09-09):** The earlier status
> overstated completion. Safety review intentionally made process creation
> opt-in and fail-closed where lifecycle or authenticated terminal integration
> is incomplete.
>
> - **Phase 1 (launch-service abstraction)** — DONE. `crates/jobwrap-daemon/
>   src/launch.rs` plus `GlobalPermission`/`authorize_global` in
>   `jobwrap-core`, `LaunchConfig`/`TerminalConfig`/`HelpConfig` in
>   `jobwrap-config`, and structured `LaunchRequest` in `jobwrap-protocol`.
> - **Phase 2 (API preview)** — NOT IMPLEMENTED for launches. Configuration
>   requiring a preview rejects execution.
> - **Phase 3 (API-managed process creation)** — DISABLED. The previous code
>   leaked child lifecycle supervision and recorded an invalid process group;
>   it was removed pending a PTY-backed supervised implementation.
> - **Phase 4 (new-terminal launching)** — DONE. `terminal.rs` provides the
>   backend abstraction (gnome-terminal, xterm), one-time launch ids, and the
>   `jobwrap attach-launch <id>` helper that retrieves the pending structured
>   request over the Unix socket and registers the job.
> - **Phase 5 (static documentation)** — PARTIAL. `docs.rs` implements bounded
>   target identification, shebang/interpreter resolution, and man-page lookup.
>   Package metadata is not implemented. Exposed as `POST /api/v1/documentation/identify`
>   and man-page endpoints, plus the CLI `inspect` subcommand.
> - **Phase 6 (restricted help probes)** — EXPERIMENTAL AND OFF BY DEFAULT.
>   `probe.rs` implements preview + execute with limited isolation (process group, timeout, output limit,
>   isolated HOME, filtered environment, closed stdin) and result
>   classification. This is not an OS sandbox. Separate global permissions per probe kind.
> - **Phase 7 (interpreter/script distinction)** — DONE. Shebang parsing and
>   interpreter resolution distinguish interpreter, executable, and script
>   probes.
> - **Phase 8 (help cache)** — DONE. Fingerprint-keyed results cached in
>   SQLite (`help_cache` table) when `[help] cache_results = true`.
> - **Phase 9 (existing-terminal execution)** — DISABLED. Terminal discovery
>   types remain, but command delivery is rejected until an authenticated
>   cooperative channel exists.
> - **Phase 10 (security hardening)** — IN PROGRESS. Authorization-matrix
>   tests, global-permission tests, and probe-classification tests are added;
>   full fuzzing/race tests remain future work.

## 1. Objective

Extend Jobwrap so that authorized users and API clients can:

* start new managed processes;
* start processes in new graphical terminal windows;
* start processes in existing Jobwrap-enabled terminals;
* inspect command documentation without executing the target;
* explicitly investigate whether an executable or script supports a help argument;
* distinguish interpreter documentation from script documentation;
* construct and submit safe, structured launch requests;
* monitor and control newly created jobs through the existing Jobwrap API and web interface.

The web interface and external API must use the same internal launch service. There must not be separate web-only and API-only process creation paths.

---

# 2. Core safety principles

The feature should be governed by the following rules:

1. Process creation is a privileged operation.
2. Help probing is also process execution and must require explicit authorization.
3. Jobwrap must assume that `--help`, `-h`, no arguments, and similar inputs may be unsafe.
4. Static documentation lookup must be preferred over execution.
5. Commands must be represented as executable paths plus argument arrays.
6. Jobwrap must not implicitly invoke a shell.
7. Every launch must have an auditable requester identity.
8. API retries must not accidentally create duplicate processes.
9. Terminal creation must not expose secrets in process arguments.
10. Existing-terminal execution must use cooperative integration where possible.
11. The daemon must revalidate authorization and terminal state immediately before execution.
12. Monitoring failures should not unnecessarily terminate an already-running user process.
13. Remote command execution must remain disabled unless explicitly enabled through configuration and permissions.
14. All launch-related configuration must have secure per-user defaults.
15. Process creation must never run as root or another Unix user in the initial implementation.

---

# 3. Unified launch service

Introduce one internal service responsible for all process creation:

```text
LaunchService
```

It should be called by:

```text
Web interface
REST API
Local CLI
Approved agent integrations
```

The launch service should centralize:

* request parsing;
* normalization;
* executable resolution;
* authorization;
* policy evaluation;
* working-directory validation;
* environment filtering;
* terminal selection;
* launch preview generation;
* launch confirmation;
* idempotency;
* process creation;
* job registration;
* audit logging;
* error reporting.

Neither HTTP handlers nor web-interface code should directly spawn processes.

Conceptually:

```text
Web UI ───────┐
API client ───┼──> LaunchService ───> Process/terminal backend
Local CLI ────┘
```

---

# 4. Launch request model

A launch request must be structured.

Example conceptual request:

```json
{
  "executable": "/usr/bin/python3",
  "arguments": [
    "/home/user/project/analysis.py",
    "--config",
    "analysis.toml"
  ],
  "working_directory": "/home/user/project",
  "terminal_target": {
    "type": "new_terminal",
    "backend": "gnome-terminal"
  },
  "job_name": "analysis-run",
  "access_profile": "private",
  "environment": {
    "mode": "filtered",
    "set": {
      "OMP_NUM_THREADS": "8"
    }
  }
}
```

The normal API must not accept this:

```json
{
  "command": "python3 analysis.py | tee output.log"
}
```

Shell syntax introduces ambiguity and significantly expands the security surface.

If shell execution is supported later, it must use:

* a distinct request type;
* a distinct permission;
* a prominent warning;
* separate audit records;
* explicit configuration enabling;
* no automatic conversion from structured requests.

---

# 5. API permissions

Process creation should be controlled through granular permissions.

Suggested permissions:

```text
LaunchManagedProcess
LaunchDetachedProcess
LaunchInNewTerminal
LaunchInExistingTerminal
UseShellExecution

SetExecutable
SetArguments
SetWorkingDirectory
SetEnvironment
SetJobName
SelectAccessProfile
SelectTerminalBackend

PreviewLaunch
ExecutePreviewedLaunch
CancelPendingLaunch

InspectStaticMetadata
InspectManPage
InspectPackageDocumentation
ProbeInterpreterHelp
ProbeExecutableHelp
ProbeScriptHelp

ViewLaunchRequests
ViewLaunchAudit
ModifyLaunchProfiles
```

A token should not automatically receive every launch-related permission.

Example agent token:

```text
PreviewLaunch
ExecutePreviewedLaunch
LaunchManagedProcess
LaunchInNewTerminal
SetArguments
SetWorkingDirectory
ViewJobStatus
ViewJobOutput
```

It may intentionally lack:

```text
UseShellExecution
SetEnvironment
LaunchInExistingTerminal
SelectAccessProfile
ProbeScriptHelp
```

---

# 6. Launch authorization

Authorization should consider more than a single role.

The decision should use:

```text
Requester identity
Token or session scopes
Requested executable
Requested working directory
Requested environment
Requested terminal target
Selected job access profile
Launch profile restrictions
Current daemon configuration
Human-confirmation requirements
```

Conceptual authorization call:

```text
authorize_launch(principal, normalized_request, policy)
```

Possible outcomes:

```text
Allowed
AllowedWithHumanConfirmation
AllowedWithRestrictedEnvironment
Denied
```

The result should explain the specific decision.

Example denial:

```text
This token may launch managed processes but may not open new graphical terminals.
```

---

# 7. Launch profiles

Add named launch profiles to the per-user configuration.

Example:

```toml
[launch_profiles.standard]
allowed_executables = "any"
allowed_working_directories = ["~/projects"]
terminal_modes = ["managed", "new-terminal"]
allow_environment_overrides = false
allow_shell = false
require_preview = true
require_human_confirmation = true
job_access_profile = "private"

[launch_profiles.agent-safe]
allowed_executables = [
    "/usr/bin/python3",
    "/usr/bin/bash",
    "/usr/bin/cargo"
]
allowed_working_directories = [
    "~/projects"
]
terminal_modes = ["managed", "new-terminal"]
allow_environment_overrides = false
allow_shell = false
require_preview = true
require_human_confirmation = false
job_access_profile = "private"

[launch_profiles.manual]
allowed_executables = "any"
allowed_working_directories = ["~"]
terminal_modes = [
    "managed",
    "new-terminal",
    "existing-terminal"
]
allow_environment_overrides = true
allow_shell = false
require_preview = true
require_human_confirmation = true
```

The daemon must resolve paths before matching them against restrictions.

The configuration should support:

```text
Allow list
Deny list
Allowed directory roots
Terminal-mode restrictions
Environment restrictions
Maximum process count
Maximum concurrent launches
Confirmation policy
Required resulting job profile
```

---

# 8. API launch workflow

The preferred API workflow should be two-phase:

```text
Preview
   ↓
Execute exact preview
```

## 8.1 Preview endpoint

```http
POST /api/v1/launch/previews
```

This endpoint must not execute the target.

It should:

* authenticate the requester;
* validate basic permissions;
* normalize paths;
* resolve the executable;
* validate the working directory;
* filter or reject environment variables;
* validate the terminal target;
* select the effective launch profile;
* calculate whether human confirmation is required;
* produce the exact structured specification;
* create a short-lived immutable preview.

Example response:

```json
{
  "preview_id": "01K...",
  "expires_at": "2026-08-03T08:00:00Z",
  "normalized_request": {
    "executable": "/usr/bin/python3.8",
    "arguments": [
      "/home/user/project/analysis.py",
      "--config",
      "analysis.toml"
    ],
    "working_directory": "/home/user/project",
    "terminal_target": {
      "type": "new_terminal",
      "backend": "gnome-terminal"
    },
    "access_profile": "private"
  },
  "authorization": {
    "status": "allowed",
    "human_confirmation_required": false
  },
  "warnings": []
}
```

The preview should be immutable. Execution must use the stored normalized specification rather than reparsing an arbitrary replacement request.

## 8.2 Execution endpoint

```http
POST /api/v1/launch/previews/{preview_id}/execute
```

The daemon should re-check:

* preview expiration;
* requester identity;
* token validity;
* authorization;
* launch-profile status;
* terminal availability;
* working-directory existence;
* executable identity;
* executable modification status where relevant;
* available concurrency quota.

If the executable changed after preview, execution should normally fail and require a new preview.

Example response:

```json
{
  "launch_id": "01K...",
  "job_id": "01K...",
  "state": "launching",
  "terminal_target": {
    "type": "new_terminal",
    "backend": "gnome-terminal"
  },
  "audit_id": "01K..."
}
```

---

# 9. Idempotency

Every API process-creation request should require an idempotency key.

Example:

```http
Idempotency-Key: e869cf82-...
```

The key should be scoped to:

```text
Authenticated principal
Endpoint
Normalized launch request or preview
Configured retention period
```

If the same client repeats the execution request with the same key, Jobwrap should return the existing launch result rather than create another process.

Possible response:

```json
{
  "job_id": "01K...",
  "state": "running",
  "idempotent_replay": true
}
```

Reusing the same key for a different request must fail.

---

# 10. Human confirmation

Some API clients, especially LLM agents, may operate without immediate human interaction.

The policy should support:

```text
Always require confirmation
Require confirmation for certain executables
Require confirmation for terminal creation
Require confirmation for existing-terminal use
Allow execution without confirmation for approved tokens and profiles
```

Human confirmation can be represented as a state on the launch preview:

```text
PendingConfirmation
Confirmed
Rejected
Expired
```

Possible endpoint:

```http
POST /api/v1/launch/previews/{preview_id}/confirm
```

Confirmation should be tied to:

* the exact preview;
* the confirming user identity;
* a short expiration;
* any changed executable or working-directory metadata.

A confirmation must not authorize later modified arguments.

---

# 11. Process launch modes

Support three initial launch modes.

## 11.1 Managed process

The process is started and owned by Jobwrap without opening a graphical terminal.

```json
{
  "terminal_target": {
    "type": "managed"
  }
}
```

Jobwrap should allocate a PTY when requested by the launch profile.

This mode is suitable for:

* automated agents;
* background jobs;
* non-interactive commands;
* commands primarily controlled from the web UI.

## 11.2 New terminal

Jobwrap opens a new terminal window or pane and runs the managed command there.

```json
{
  "terminal_target": {
    "type": "new_terminal",
    "backend": "gnome-terminal"
  }
}
```

This is appropriate when the user wants a visible interactive process.

## 11.3 Existing terminal

Jobwrap starts the command in an existing registered terminal.

```json
{
  "terminal_target": {
    "type": "existing_terminal",
    "terminal_id": "term-01K..."
  }
}
```

This mode should have the strongest restrictions and should be disabled by default for external API tokens.

---

# 12. New-terminal architecture

The daemon should not pass the full command and arguments directly to the terminal emulator.

Preferred sequence:

1. API execution is authorized.
2. The daemon stores an immutable pending launch request.
3. The daemon creates a random, one-time launch capability.
4. The daemon launches the terminal emulator with:

```text
jobwrap attach-launch <one-time-id>
```

5. The helper connects through the private Unix socket.
6. The daemon verifies:

   * peer UID;
   * one-time ID;
   * expiration;
   * expected launch state.
7. The helper retrieves the structured launch request.
8. The helper creates the child process.
9. The job registers with the daemon.
10. The one-time launch ID is invalidated.

This avoids exposing:

* sensitive arguments;
* environment values;
* API tokens;
* access-policy details;
* long shell command strings.

The one-time ID must:

* contain sufficient entropy;
* expire quickly;
* be valid once;
* be bound to the current user;
* not be accepted over the HTTP API;
* never grant unrelated daemon access.

---

# 13. Terminal backend abstraction

Define a terminal backend interface conceptually similar to:

```text
TerminalBackend
    identifier
    availability check
    supported capabilities
    launch new terminal
    launch new tab
    launch new pane
    report launch errors
```

Initial backends might include:

```text
GNOME Terminal
kitty
WezTerm
Konsole
Xfce Terminal
xterm-compatible fallback
```

Backend selection should come from per-user config.

Example:

```toml
[terminal]
preferred_backend = "gnome-terminal"
allow_api_override = false
```

API clients should not freely select arbitrary terminal executables or templates.

If override is allowed, it should select from configured backend identifiers only.

---

# 14. Existing-terminal execution

Execution in an existing terminal should only target terminals registered with Jobwrap.

Each registered terminal should expose:

```text
Terminal ID
Owner UID
Shell type
Current working directory
Environment identity
Ready/busy state
Foreground program
Registration time
Last heartbeat
Supported launch mechanism
```

Terminal states:

```text
Ready
Busy
InteractiveApplication
Disconnected
Unknown
```

The API should list only terminals visible to the requester:

```http
GET /api/v1/terminals
```

Before executing, the daemon must revalidate that the terminal remains `Ready`.

If not:

```http
409 Conflict
```

```json
{
  "error": {
    "code": "terminal_not_ready",
    "message": "The selected terminal is no longer ready to accept a command."
  }
}
```

No command should be queued automatically unless queueing is explicitly designed and requested.

---

# 15. Cooperative shell integration

The preferred existing-terminal mechanism should use a cooperative shell hook.

The hook should:

* notify Jobwrap when the shell prompt is displayed;
* mark the terminal busy before command execution;
* report the current working directory;
* optionally report a filtered environment identity;
* accept structured launch requests;
* render the command in the terminal before execution;
* optionally require local confirmation;
* execute without reconstructing an unsafe shell string where possible.

This is more reliable than injecting text into the terminal PTY.

Terminal-emulator remote-control APIs may be supported as a fallback, but should be classified as lower assurance.

---

# 16. Static command identification

Before documentation lookup or help probing, Jobwrap should inspect the selected target without executing it.

Collect:

```text
Original path
Resolved path
File type
Executable bit
Symlink chain
File owner and mode
ELF or script identification
Shebang
Detected interpreter
Package ownership
Modification time
File fingerprint
```

Example:

```text
Selected target:
  /home/user/project/analyse.py

Type:
  Text script

Shebang:
  /usr/bin/env python3

Resolved interpreter:
  /usr/bin/python3.8
```

Static identification must remain available to users who are not authorized to execute help probes.

---

# 17. Documentation discovery hierarchy

Documentation discovery should proceed from lower-risk to higher-risk operations.

```text
1. Static metadata
2. Man-page lookup
3. Package documentation
4. Interpreter documentation
5. Explicit target help probe
6. Custom probe
```

The UI and API must expose these as separate operations.

---

# 18. Man-page inspection

Man-page lookup should not execute the selected target.

Potential inputs:

```text
Target basename
Resolved executable basename
Package name
Interpreter basename
Explicit man section
```

Conceptual API:

```http
POST /api/v1/documentation/man-pages/search
```

Request:

```json
{
  "target": "/usr/bin/python3"
}
```

Response:

```json
{
  "matches": [
    {
      "name": "python3",
      "section": "1",
      "relationship": "resolved-executable",
      "content_available": true
    }
  ]
}
```

Another endpoint can return formatted content:

```http
GET /api/v1/documentation/man-pages/python3/1
```

Jobwrap should show when the man page name is not an exact match for the executable.

For example:

```text
Executable:
  /usr/bin/python3.8

Documentation:
  python3(1)
```

---

# 19. Package documentation

Where supported, Jobwrap may inspect:

```text
Owning package
Package version
Package description
README files
Examples
Info pages
Installed documentation paths
```

This is static inspection and should not execute the target.

It should remain optional and platform-specific.

---

# 20. Help probing policy

Jobwrap must default to:

```text
Help support unknown
```

It must not assume that any of these are safe:

```text
--help
-h
help
/?
No arguments
```

The web UI should provide an explicit action:

```text
Investigate help option
```

The API should use a separate probe endpoint.

```http
POST /api/v1/documentation/help-probes/previews
```

This endpoint should produce the exact proposed probe without executing it.

Example:

```json
{
  "target": "/home/user/project/analyse.py",
  "probe_argument": "--help",
  "interpreter": "/usr/bin/python3.8"
}
```

Normalized preview:

```json
{
  "preview_id": "01K...",
  "invocation": {
    "executable": "/usr/bin/python3.8",
    "arguments": [
      "/home/user/project/analyse.py",
      "--help"
    ]
  },
  "risk": {
    "executes_target": true,
    "help_support_known": false,
    "warning": "The target may ignore --help or perform normal work."
  }
}
```

Execution endpoint:

```http
POST /api/v1/documentation/help-probes/{preview_id}/execute
```

Help probing requires its own idempotency key.

---

# 21. Probe permissions

Use separate permissions for different probes:

```text
ProbeInterpreterHelp
ProbeExecutableHelp
ProbeScriptHelp
ProbeCustomArguments
```

A token allowed to inspect man pages should not automatically be able to execute help probes.

The default should be:

```text
Man-page inspection: authenticated
Interpreter help: controller or owner
Executable help probe: owner
Script help probe: owner
Custom probe: disabled
```

Profiles may loosen these restrictions for trusted agents.

---

# 22. Probe sandbox

Help probes should run in a more restrictive environment than normal jobs.

Desired restrictions:

```text
Short timeout
Output-size limit
No interactive input
Dedicated process group
Isolated temporary working directory
Isolated HOME
Filtered environment
No inherited credentials
Network disabled where enforceable
CPU limit
Memory limit
Process-count limit
Cleanup after execution
```

Suggested defaults:

```text
Timeout: 3 seconds
Maximum configurable timeout: 15 seconds
Output limit: 1 MiB
Standard input: closed
Working directory: isolated temporary directory
HOME: isolated temporary directory
Environment: allow-listed
Network: disabled when sandbox backend supports it
Termination: SIGTERM followed by SIGKILL
```

Potential environment allow list:

```text
LANG
LC_ALL
TERM
TZ
```

Do not inherit:

```text
SSH_AUTH_SOCK
AWS credentials
Cloud API tokens
Git credentials
Desktop session secrets
Project-specific secrets
```

The system must clearly state which restrictions are actually enforced on the current platform.

---

# 23. Probe sandbox backends

The plan should allow several conceptual sandbox implementations:

```text
Basic process-group sandbox
systemd-run --user transient unit
bubblewrap
Landlock where suitable
Linux namespaces
Container backend
```

The initial implementation may start with:

* dedicated process group;
* timeout;
* resource limits;
* isolated temporary directory;
* filtered environment;
* closed standard input.

Stronger filesystem and network isolation can then be added as an explicit backend.

The UI and API should report the active protection level:

```json
{
  "sandbox": {
    "level": "basic",
    "network_isolated": false,
    "filesystem_isolated": true,
    "environment_filtered": true
  }
}
```

---

# 24. Interpreter and script distinction

For interpreted scripts, documentation must be divided into separate sources.

For Python:

```text
Python man page
Python interpreter --help
Script --help probe
```

These must be independent operations.

Given:

```text
#!/usr/bin/env python3
```

Jobwrap should resolve and display the interpreter used in the selected launch environment.

Example:

```text
Shebang:
  /usr/bin/env python3

Resolved interpreter:
  /usr/bin/python3.8
```

The API should model this explicitly:

```json
{
  "target_type": "script",
  "script": "/home/user/project/analyse.py",
  "interpreter": {
    "executable": "/usr/bin/python3.8",
    "prefix_arguments": []
  }
}
```

The script help probe would execute:

```text
/usr/bin/python3.8 /home/user/project/analyse.py --help
```

The interpreter help probe would execute:

```text
/usr/bin/python3.8 --help
```

These results must be stored separately.

---

# 25. Shell scripts

Shell scripts must be executed as child processes for help probing.

Do not source scripts.

Forbidden probe behavior:

```text
source script.sh --help
```

Preferred:

```text
/bin/bash /path/to/script.sh --help
```

The selected interpreter should come from:

* the shebang;
* explicit user selection;
* a trusted launch profile.

Do not infer an interpreter solely from the filename extension without showing the assumption.

---

# 26. Help probe result model

A probe result should include:

```text
Exact invocation
Requester
Target fingerprint
Interpreter fingerprint
Start and end time
Duration
Exit status
Termination signal
Captured stdout
Captured stderr
Output truncation status
Timeout status
Sandbox level
Files created in the sandbox
Files modified in the sandbox
Child process count
Blocked operations where observable
Classification
Warnings
```

Possible classifications:

```text
LikelyHelpOutput
PossibleHelpOutput
ArgumentRejected
NoOutput
NormalProgramBehaviorSuspected
TimedOut
SideEffectsObserved
ExecutionFailed
Unknown
```

Exit status zero must not automatically mean that `--help` is supported.

Good wording:

```text
The output appears to contain usage documentation.
```

Avoid:

```text
This program safely supports --help.
```

---

# 27. Help result caching

A help result may be cached only with sufficient provenance.

Cache key inputs should include:

```text
Target resolved path
Target fingerprint
Target modification time
Interpreter resolved path
Interpreter fingerprint
Probe argument
Relevant launch environment identity
Sandbox profile
```

Cached result:

```text
Probe timestamp
Exact invocation
Classification
Output digest
User confirmation
Warnings
```

Invalidate the result when:

```text
Target changes
Interpreter changes
Probe argument changes
Sandbox policy changes materially
Environment identity changes materially
User clears the result
```

The UI should say:

```text
Previously investigated on August 3, 2026.
The target file has not changed since that probe.
```

Do not store a simple permanent boolean such as:

```text
supports_help = true
```

---

# 28. API documentation discovery workflow

Recommended API sequence:

```text
POST identify target
GET or POST man-page lookup
Optionally preview interpreter help probe
Optionally execute interpreter help probe
Optionally preview script help probe
Optionally execute script help probe
Build launch request
Preview launch
Execute preview
```

Conceptual endpoints:

```http
POST /api/v1/documentation/identify
POST /api/v1/documentation/man-pages/search
GET  /api/v1/documentation/man-pages/{name}/{section}

POST /api/v1/documentation/help-probes/previews
POST /api/v1/documentation/help-probes/{preview_id}/execute
GET  /api/v1/documentation/help-probes/{probe_id}
DELETE /api/v1/documentation/help-probes/{probe_id}
```

---

# 29. API launch endpoint summary

Suggested launch endpoints:

```http
GET  /api/v1/launch/capabilities
GET  /api/v1/launch/profiles
POST /api/v1/launch/previews
GET  /api/v1/launch/previews/{preview_id}
POST /api/v1/launch/previews/{preview_id}/confirm
POST /api/v1/launch/previews/{preview_id}/execute
DELETE /api/v1/launch/previews/{preview_id}

GET  /api/v1/launches/{launch_id}
GET  /api/v1/jobs/{job_id}
```

Terminal endpoints:

```http
GET /api/v1/terminals
GET /api/v1/terminals/{terminal_id}
```

The API should never expose arbitrary terminal-emulator argument templates to ordinary clients.

---

# 30. API error behavior

Use stable error identifiers.

Examples:

```text
launch_not_authorized
preview_expired
preview_already_executed
executable_changed
working_directory_missing
working_directory_denied
terminal_backend_unavailable
terminal_not_ready
terminal_disconnected
environment_variable_denied
human_confirmation_required
idempotency_conflict
launch_capacity_exceeded
help_probe_not_authorized
probe_timed_out
sandbox_unavailable
```

Example response:

```json
{
  "error": {
    "code": "human_confirmation_required",
    "message": "This launch must be confirmed by the local owner.",
    "preview_id": "01K...",
    "request_id": "01K..."
  }
}
```

Internal Rust errors and stack traces must not be exposed.

---

# 31. API rate and resource limits

Process creation endpoints need stronger limits than read endpoints.

Apply limits per:

```text
Principal
Token
Source address
Launch profile
User daemon
```

Possible limits:

```text
Launch previews per minute
Launch executions per minute
Concurrent running jobs
Concurrent pending launches
Concurrent help probes
Maximum probe output
Maximum preview lifetime
Maximum arguments
Maximum argument length
Maximum environment entries
```

When limits are reached, reject before process creation.

---

# 32. Audit model

Every preview, confirmation, launch, rejection, and probe should be audited.

Launch audit record:

```text
Requester identity
Token or session identifier
Source address
Launch profile
Executable
Resolved executable
Arguments
Redacted arguments
Working directory
Environment policy
Terminal target
Preview ID
Idempotency key digest
Authorization decision
Confirmation identity
Job ID
Result
Timestamp
```

Help probe audit record:

```text
Requester identity
Target
Resolved target
Interpreter
Probe arguments
Sandbox profile
Timeout
Output limit
Result classification
Observed side effects
Timestamp
```

Secrets should never be stored in plaintext audit records.

The launch form and API should support explicit sensitive argument marking.

Example conceptual request:

```json
{
  "arguments": [
    {
      "value": "--token",
      "sensitive": false
    },
    {
      "value": "secret-value",
      "sensitive": true
    }
  ]
}
```

Internally, process execution still receives the plain value, while logs and audit records contain a redacted representation.

---

# 33. Environment handling

Environment inheritance is sensitive and should be policy controlled.

Modes:

```text
Minimal
Filtered inheritance
Explicit only
Full inheritance
```

Recommended defaults:

```text
Local manual launch: filtered inheritance
API agent launch: explicit only or restricted filtered inheritance
Help probe: minimal
```

Always prohibit API clients from setting dangerous variables unless specifically allowed.

Examples requiring caution:

```text
LD_PRELOAD
LD_LIBRARY_PATH
PYTHONPATH
PERL5LIB
RUBYLIB
NODE_OPTIONS
BASH_ENV
ENV
PROMPT_COMMAND
GIT_SSH_COMMAND
SSH_AUTH_SOCK
```

The daemon should use an allow list or named environment profile rather than ad hoc filtering alone.

---

# 34. Working-directory restrictions

API tokens and launch profiles should be restricted to configured directory roots.

Example:

```toml
[launch_profiles.agent-safe]
allowed_working_directories = [
    "/home/user/projects"
]
```

Validation should:

* normalize the path;
* resolve symlinks according to policy;
* reject nonexistent directories unless explicitly allowed;
* reject directories outside permitted roots;
* reject paths that change between preview and execution;
* avoid relying on string-prefix comparison.

For example, `/home/user/projects-old` must not match `/home/user/projects`.

---

# 35. Executable restrictions

Launch profiles should support:

```text
Any executable
Explicit executable allow list
Executable directory roots
Package-owned executables only
Script execution enabled or disabled
Interpreter allow list
Executable fingerprint pinning
```

Example:

```toml
[launch_profiles.agent-safe]
allowed_executables = [
    "/usr/bin/python3",
    "/usr/bin/cargo",
    "/usr/bin/bash"
]

allowed_script_roots = [
    "/home/user/projects"
]
```

For scripts, authorization should consider both:

```text
Interpreter
Script path
```

Allowing `/usr/bin/python3` must not automatically allow it to execute every file on the system.

---

# 36. Launch lifecycle

Use an explicit state machine:

```text
Draft
Previewed
AwaitingConfirmation
Confirmed
Launching
TerminalStarting
WrapperConnecting
ProcessStarting
Running
Failed
Cancelled
Expired
```

Each transition should be validated centrally.

Example:

```text
Previewed → Confirmed
Confirmed → Launching
Launching → TerminalStarting
TerminalStarting → WrapperConnecting
WrapperConnecting → ProcessStarting
ProcessStarting → Running
```

Error states must identify whether a process was actually created.

This distinction is critical:

```text
Launch failed before process creation
Launch failed after process creation
Process started but terminal registration failed
```

---

# 37. Failure handling

## Terminal emulator unavailable

Return an error without falling back silently.

```text
The configured terminal backend is unavailable.
No process was started.
```

## Existing terminal becomes busy

Return `409 Conflict`.

No text or input should be sent.

## Preview expires

Require a new preview.

## Executable changes after preview

Reject execution and require a new preview.

## Wrapper cannot register

The launch helper should terminate before starting the requested command where possible.

If the command has already started, it must register a degraded or orphaned state and report it prominently.

## Daemon crashes after process creation

The wrapper should continue managing the process and attempt reconnection.

## API client disconnects

The launch operation should not be cancelled merely because the HTTP client disconnects after an accepted request.

The API client should be able to query the launch using the idempotency key or launch ID.

---

# 38. Configuration additions

Conceptual per-user configuration:

```toml
[launch]
enabled = true
default_profile = "standard"
default_terminal_mode = "new-terminal"
require_preview = true
require_idempotency_key = true
allow_shell_execution = false
maximum_concurrent_jobs = 20
maximum_pending_launches = 20

[launch.api]
enabled = true
require_structured_commands = true
default_human_confirmation = true
preview_lifetime_seconds = 300
idempotency_retention_hours = 24

[terminal]
preferred_backend = "gnome-terminal"
allow_api_backend_selection = false

[help]
assume_help_available = false
prefer_man_pages = true
allow_interpreter_probes = true
allow_script_probes = true
default_probe_argument = "--help"
probe_timeout_seconds = 3
probe_output_limit_bytes = 1048576
cache_results = true

[help.sandbox]
backend = "basic"
isolated_home = true
isolated_working_directory = true
network = "disabled-if-supported"
inherit_environment = false
```

Risky combinations should generate hard errors or strong warnings.

For example:

```text
API launch enabled
+ non-loopback HTTP binding
+ shell execution enabled
+ no authentication
```

must never be accepted.

---

# 39. Web-interface workflow

The web interface should behave as a client of the same API.

Suggested flow:

```text
Select executable or script
        ↓
Inspect static metadata
        ↓
View available man pages
        ↓
Optionally inspect interpreter help
        ↓
Optionally investigate target --help
        ↓
Build structured arguments
        ↓
Select working directory
        ↓
Select launch profile
        ↓
Select managed/new/existing terminal
        ↓
Preview exact launch
        ↓
Confirm where required
        ↓
Execute preview
        ↓
Open Jobwrap job page
```

The web interface must not bypass API authorization because it runs locally.

---

# 40. Agent-oriented workflow

A trusted LLM agent should be able to:

1. Identify a target.
2. Inspect man pages.
3. Request a help-probe preview.
4. Execute the probe when authorized.
5. Read the result.
6. Construct a structured launch request.
7. Request a launch preview.
8. Review warnings and normalized paths.
9. Execute the preview.
10. Monitor the resulting job.

The agent must not be allowed to silently switch to shell mode.

A recommended token profile:

```text
InspectStaticMetadata
InspectManPage
ProbeInterpreterHelp
ProbeScriptHelp
PreviewLaunch
ExecutePreviewedLaunch
LaunchManagedProcess
LaunchInNewTerminal
SetArguments
SetWorkingDirectory
ViewJobStatus
ViewJobOutput
SendInterrupt
```

Not included by default:

```text
UseShellExecution
SetArbitraryEnvironment
LaunchInExistingTerminal
SendKill
ModifyLaunchProfiles
```

---

# 41. Implementation phases

## Phase 1: launch-service abstraction

Implement:

* structured launch request types;
* normalized launch specification;
* centralized authorization;
* launch profiles;
* audit model;
* launch state machine.

No new process creation endpoint yet.

## Phase 2: API preview

Implement:

* `POST /launch/previews`;
* immutable preview storage;
* executable and directory resolution;
* environment policy;
* permission evaluation;
* expiration;
* clear warnings and errors.

## Phase 3: API-managed process creation

Implement:

* preview execution endpoint;
* idempotency;
* managed process mode;
* job registration;
* concurrency limits;
* lifecycle and audit records.

## Phase 4: new-terminal launching

Implement:

* terminal backend abstraction;
* GNOME Terminal backend;
* one-time launch IDs;
* launch helper;
* wrapper registration;
* failure reconciliation.

## Phase 5: static documentation

Implement:

* target identification;
* shebang parsing;
* interpreter resolution;
* man-page lookup;
* package metadata where available.

## Phase 6: restricted help probes

Implement:

* probe previews;
* separate probe permissions;
* timeout;
* output limits;
* filtered environment;
* isolated temporary directories;
* process-group cleanup;
* result classification;
* audit records.

## Phase 7: interpreter/script distinction

Implement:

* interpreter documentation;
* interpreter help probe;
* script help probe;
* Python-specific user experience;
* generic shebang adapters.

## Phase 8: help cache

Implement:

* file fingerprints;
* interpreter fingerprints;
* cache invalidation;
* probe history;
* user-approved help invocation records.

## Phase 9: existing-terminal execution

Implement:

* terminal registration;
* shell-ready protocol;
* API terminal listing;
* readiness revalidation;
* cooperative launch execution;
* conflict handling.

## Phase 10: security hardening

Perform:

* authorization matrix testing;
* idempotency race testing;
* terminal-state race testing;
* executable replacement testing;
* path traversal and symlink testing;
* API replay testing;
* token revocation testing;
* output and process exhaustion testing;
* sandbox escape review;
* audit-redaction review.

---

# 42. Acceptance scenarios

## API-managed process launch

1. Agent submits a structured preview request.
2. Daemon resolves executable and working directory.
3. Agent receives immutable preview.
4. Agent executes preview with an idempotency key.
5. Daemon starts exactly one process.
6. Repeating the request returns the same Job ID.
7. Job appears in the normal Jobwrap job list.

## API new-terminal launch

1. Agent requests a new terminal.
2. Token has `LaunchInNewTerminal`.
3. Daemon creates a one-time launch record.
4. Terminal opens with the Jobwrap launch helper.
5. Helper retrieves the launch request through the Unix socket.
6. Process starts and registers.
7. No sensitive arguments appear in the terminal emulator command line.

## Unauthorized terminal launch

1. Token may launch managed processes.
2. Token requests a new terminal.
3. Preview or execution fails with `launch_not_authorized`.
4. No terminal or process is created.

## Duplicate API request

1. Client sends an execute request.
2. Network response is lost.
3. Client retries with the same idempotency key.
4. Daemon returns the existing Job ID.
5. No duplicate process is created.

## Changed executable

1. User previews a launch.
2. Executable is replaced before execution.
3. Execution fails with `executable_changed`.
4. A new preview is required.

## Man-page inspection

1. Client identifies `/usr/bin/grep`.
2. Jobwrap locates `grep(1)`.
3. Documentation is returned.
4. `grep --help` is not executed.

## Script probe

1. Client selects a Python script.
2. Jobwrap identifies Python separately from the script.
3. Client previews `python3 script.py --help`.
4. Probe executes in restricted mode.
5. Result does not automatically assert that `--help` is safe.

## Existing terminal race

1. Client previews launch into a ready terminal.
2. Terminal becomes busy.
3. Execution returns `terminal_not_ready`.
4. No command is delivered.

---

# 43. Version-one non-goals

Do not initially support:

```text
Arbitrary shell command strings through the API
Automatic no-argument execution for documentation discovery
Automatic probing of every common help flag
Launch as root
Launch as another Unix user
sudo
Remote editing of terminal backend templates
Automatic injection into unrelated terminal PTYs
Public unauthenticated process creation
Unlimited environment inheritance
Internet-facing deployment without an external secure transport layer
Automatic queueing into busy terminals
Automatic interpretation of help output as unquestionably correct
```

---

# 44. Central architectural requirement

The most important implementation rule is:

> Web users, local CLI users, and API clients must all use the same normalized, authorized, audited launch service.

A process should never be created merely because an HTTP handler, web button, or terminal backend decided to call an operating-system spawn function directly.

The intended control flow is:

```text
Request
  ↓
Authentication
  ↓
Authorization
  ↓
Normalization
  ↓
Immutable preview
  ↓
Optional human confirmation
  ↓
Final revalidation
  ↓
Idempotency check
  ↓
Process or terminal creation
  ↓
Job registration
  ↓
Audit record
```

The documentation flow should remain separately staged:

```text
Static inspection
  ↓
Man-page inspection
  ↓
Optional interpreter probe
  ↓
Optional target help probe
  ↓
Structured launch preview
  ↓
Authorized execution
```
