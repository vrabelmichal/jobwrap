//! Authorization vocabulary.
//!
//! Authentication ("who are you") and authorization ("may you do this") are
//! separate. Every HTTP handler, WebSocket action and local command routes
//! authorization through [`authorize`].

use serde::{Deserialize, Serialize};

use crate::job::JobRecord;

/// The permissions a principal can be granted for a job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
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
    Launch,
}

impl Permission {
    /// A human-readable description.
    pub fn describe(self) -> &'static str {
        match self {
            Permission::ViewStatus => "view job status",
            Permission::ViewOutput => "view terminal output",
            Permission::ViewCommand => "view the command line",
            Permission::ViewWorkingDirectory => "view the working directory",
            Permission::SendInput => "send terminal input",
            Permission::SendInterrupt => "send SIGINT",
            Permission::SendTerminate => "send SIGTERM",
            Permission::SendStop => "send SIGSTOP",
            Permission::SendContinue => "send SIGCONT",
            Permission::SendKill => "send SIGKILL",
            Permission::Restart => "restart the job",
            Permission::Delete => "delete the job record",
            Permission::DownloadLogs => "download the output log",
            Permission::ModifyPolicy => "change the access policy",
            Permission::Launch => "launch a managed process",
        }
    }
}

/// Global (non-job-specific) permissions for operations such as launching and
/// documentation inspection. These are authorized by [`authorize_global`]
/// rather than against a particular job record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalPermission {
    LaunchManagedProcess,
    LaunchInNewTerminal,
    LaunchInExistingTerminal,
    UseShellExecution,
    ProbeInterpreterHelp,
    ProbeExecutableHelp,
    ProbeScriptHelp,
    InspectStaticMetadata,
    InspectManPage,
}

impl GlobalPermission {
    /// A human-readable description.
    pub fn describe(self) -> &'static str {
        match self {
            GlobalPermission::LaunchManagedProcess => "launch a managed process",
            GlobalPermission::LaunchInNewTerminal => "launch in a new terminal",
            GlobalPermission::LaunchInExistingTerminal => "launch in an existing terminal",
            GlobalPermission::UseShellExecution => "execute a shell command string",
            GlobalPermission::ProbeInterpreterHelp => "probe an interpreter's help",
            GlobalPermission::ProbeExecutableHelp => "probe an executable's help",
            GlobalPermission::ProbeScriptHelp => "probe a script's help",
            GlobalPermission::InspectStaticMetadata => "inspect static file metadata",
            GlobalPermission::InspectManPage => "inspect man pages",
        }
    }

    /// The default access tier required for this permission.
    pub fn default_level(self) -> AccessLevel {
        match self {
            GlobalPermission::InspectStaticMetadata | GlobalPermission::InspectManPage => {
                AccessLevel::Authenticated
            }
            GlobalPermission::ProbeInterpreterHelp
            | GlobalPermission::LaunchManagedProcess
            | GlobalPermission::LaunchInNewTerminal => AccessLevel::Controller,
            GlobalPermission::ProbeExecutableHelp
            | GlobalPermission::ProbeScriptHelp
            | GlobalPermission::LaunchInExistingTerminal => AccessLevel::Owner,
            GlobalPermission::UseShellExecution => AccessLevel::Disabled,
        }
    }
}

/// Authorize a global (non-job) operation.
pub fn authorize_global(
    principal: &Principal,
    permission: GlobalPermission,
) -> AuthorizationDecision {
    let required = permission.default_level();
    if required == AccessLevel::Disabled {
        return AuthorizationDecision::Disabled;
    }

    // API tokens: grants are the authority.
    if let Principal::ApiToken { grants, .. } = principal {
        if grants.iter().any(|g| g.global.contains(&permission)) {
            return AuthorizationDecision::Allow;
        }
        return AuthorizationDecision::Deny { required };
    }

    // Owner-tier operations require the local Unix user.
    if required == AccessLevel::Owner {
        return match principal {
            Principal::LocalUnixUser { .. } => AuthorizationDecision::Allow,
            _ => AuthorizationDecision::Deny { required },
        };
    }

    let tier = principal_tier(principal);
    if tier >= required_level_tier(required) {
        AuthorizationDecision::Allow
    } else {
        AuthorizationDecision::Deny { required }
    }
}

/// The named access tiers a profile can assign to a permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
    /// Available to unauthenticated principals.
    Public,
    /// Available to any authenticated principal (session or token).
    Authenticated,
    /// Available to principals that hold the controller role (e.g. the global
    /// browser-session role).
    Controller,
    /// Available only to the local owner (the Unix user who launched the job).
    Owner,
    /// Denied to everyone.
    Disabled,
}

impl AccessLevel {
    /// Whether this tier is at least as permissive as `other`.
    pub fn grants(&self, other: AccessLevel) -> bool {
        // Disabled grants nothing.
        if *self == AccessLevel::Disabled {
            return false;
        }
        if other == AccessLevel::Disabled {
            return true;
        }
        self >= &other
    }

    /// The stable lowercase name used in configuration and APIs.
    pub fn as_str(self) -> &'static str {
        match self {
            AccessLevel::Public => "public",
            AccessLevel::Authenticated => "authenticated",
            AccessLevel::Controller => "controller",
            AccessLevel::Owner => "owner",
            AccessLevel::Disabled => "disabled",
        }
    }
}

impl std::fmt::Display for AccessLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The access policy snapshot attached to a job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessPolicy {
    pub status: AccessLevel,
    pub output: AccessLevel,
    pub command: AccessLevel,
    pub working_directory: AccessLevel,
    pub send_input: AccessLevel,
    pub signal_interrupt: AccessLevel,
    pub signal_terminate: AccessLevel,
    pub signal_stop: AccessLevel,
    pub signal_continue: AccessLevel,
    pub signal_kill: AccessLevel,
    pub restart: AccessLevel,
    pub delete: AccessLevel,
    pub launch: AccessLevel,
}

impl AccessPolicy {
    /// The access level required for a permission under this policy.
    pub fn required_for(&self, permission: Permission) -> RequiredAccess {
        let level = match permission {
            Permission::ViewStatus => self.status,
            Permission::ViewOutput => self.output,
            Permission::ViewCommand => self.command,
            Permission::ViewWorkingDirectory => self.working_directory,
            Permission::SendInput => self.send_input,
            Permission::SendInterrupt => self.signal_interrupt,
            Permission::SendTerminate => self.signal_terminate,
            Permission::SendStop => self.signal_stop,
            Permission::SendContinue => self.signal_continue,
            Permission::SendKill => self.signal_kill,
            Permission::Restart => self.restart,
            Permission::Delete => self.delete,
            Permission::DownloadLogs => self.output,
            Permission::ModifyPolicy => AccessLevel::Owner,
            Permission::Launch => self.launch,
        };
        RequiredAccess { level }
    }
}

/// The resolved requirement for a single permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequiredAccess {
    pub level: AccessLevel,
}

/// Who is making the request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// No credentials supplied.
    Anonymous,
    /// A logged-in browser session with the controller role.
    BrowserSession { session_id: String },
    /// A scoped API token.
    ApiToken {
        token_id: String,
        grants: Vec<TokenGrant>,
    },
    /// The local Unix user communicating over the private socket.
    LocalUnixUser { uid: u32 },
}

/// A structured grant derived from a token's scopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenGrant {
    /// Which jobs this grant applies to.
    pub resource: ResourceSelector,
    /// The permissions granted for matching jobs.
    pub permissions: std::collections::BTreeSet<Permission>,
    /// Global (non-job-specific) permissions granted by the token.
    pub global: std::collections::BTreeSet<GlobalPermission>,
}

/// Which jobs a token grant applies to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceSelector {
    /// Any job owned by the token's owner.
    AllOwned,
    /// A single named job.
    Job(crate::JobId),
}

/// The outcome of an authorization check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorizationDecision {
    /// The request is permitted.
    Allow,
    /// The request is denied because the principal is not authenticated enough.
    /// `required` names the tier that would be needed.
    Deny { required: AccessLevel },
    /// The permission is disabled for this job entirely.
    Disabled,
}

/// A profile's access configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileAccess {
    pub status: AccessLevel,
    pub output: AccessLevel,
    pub command: AccessLevel,
    pub working_directory: AccessLevel,
    pub send_input: AccessLevel,
    pub signal_interrupt: AccessLevel,
    pub signal_terminate: AccessLevel,
    pub signal_stop: AccessLevel,
    pub signal_continue: AccessLevel,
    pub signal_kill: AccessLevel,
    pub restart: AccessLevel,
    pub delete: AccessLevel,
    pub launch: AccessLevel,
}

impl ProfileAccess {
    /// Convert into a job [`AccessPolicy`].
    pub fn into_policy(self) -> AccessPolicy {
        AccessPolicy {
            status: self.status,
            output: self.output,
            command: self.command,
            working_directory: self.working_directory,
            send_input: self.send_input,
            signal_interrupt: self.signal_interrupt,
            signal_terminate: self.signal_terminate,
            signal_stop: self.signal_stop,
            signal_continue: self.signal_continue,
            signal_kill: self.signal_kill,
            restart: self.restart,
            delete: self.delete,
            launch: self.launch,
        }
    }
}

/// The identity of a principal expressed as an access tier, used by the
/// authorization function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PrincipalTier {
    Anonymous,
    Authenticated,
    Controller,
    Owner,
}

fn principal_tier(principal: &Principal) -> PrincipalTier {
    match principal {
        Principal::Anonymous => PrincipalTier::Anonymous,
        Principal::BrowserSession { .. } => PrincipalTier::Controller,
        Principal::ApiToken { .. } => PrincipalTier::Authenticated,
        Principal::LocalUnixUser { .. } => PrincipalTier::Owner,
    }
}

fn is_owner(principal: &Principal, job: &JobRecord) -> bool {
    match principal {
        Principal::LocalUnixUser { uid } => job.owner_uid == *uid,
        _ => false,
    }
}

fn token_can(principal: &Principal, job: &JobRecord, permission: Permission) -> bool {
    let Principal::ApiToken { grants, .. } = principal else {
        return false;
    };
    grants.iter().any(|grant| {
        let resource_ok = match grant.resource {
            ResourceSelector::AllOwned => true,
            ResourceSelector::Job(id) => id == job.id,
        };
        resource_ok && grant.permissions.contains(&permission)
    })
}

/// The single authorization entry point used by every handler.
pub fn authorize(
    principal: &Principal,
    job: &JobRecord,
    permission: Permission,
) -> AuthorizationDecision {
    let required = job.access_policy.required_for(permission);

    if required.level == AccessLevel::Disabled {
        return AuthorizationDecision::Disabled;
    }
    // Public information is available to everyone regardless of identity.
    if required.level == AccessLevel::Public {
        return AuthorizationDecision::Allow;
    }

    // API tokens are authenticated but their scopes are the authority: a
    // token may perform an action only when it holds the matching grant for
    // this job. Role ordering does not grant tokens extra control.
    if let Principal::ApiToken { .. } = principal {
        if token_can(principal, job, permission) {
            return AuthorizationDecision::Allow;
        }
        return AuthorizationDecision::Deny {
            required: required.level,
        };
    }

    let tier = principal_tier(principal);

    // Owner is a specific identity, not just the top of a role ordering.
    if required.level == AccessLevel::Owner {
        if tier == PrincipalTier::Owner && is_owner(principal, job) {
            return AuthorizationDecision::Allow;
        }
        return AuthorizationDecision::Deny {
            required: required.level,
        };
    }

    if tier >= required_level_tier(required.level) {
        AuthorizationDecision::Allow
    } else {
        AuthorizationDecision::Deny {
            required: required.level,
        }
    }
}

fn required_level_tier(level: AccessLevel) -> PrincipalTier {
    match level {
        AccessLevel::Public => PrincipalTier::Anonymous,
        AccessLevel::Authenticated => PrincipalTier::Authenticated,
        AccessLevel::Controller => PrincipalTier::Controller,
        AccessLevel::Owner => PrincipalTier::Owner,
        AccessLevel::Disabled => PrincipalTier::Anonymous,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{JobId, JobName, JobState};
    use std::str::FromStr;

    fn owner_record() -> JobRecord {
        JobRecord {
            id: JobId::default(),
            display_name: JobName::new("test").expect("name"),
            owner_uid: 1000,
            wrapper_pid: None,
            child_pid: None,
            process_group_id: None,
            command: crate::job::CommandDisplay::new(vec!["python3".to_string()]).expect("command"),
            executable: std::path::PathBuf::from("/usr/bin/python3"),
            arguments: Vec::new(),
            working_directory: std::path::PathBuf::from("/tmp"),
            profile_name: "standard".to_string(),
            access_policy: AccessPolicy {
                status: AccessLevel::Public,
                output: AccessLevel::Public,
                command: AccessLevel::Authenticated,
                working_directory: AccessLevel::Authenticated,
                send_input: AccessLevel::Controller,
                signal_interrupt: AccessLevel::Controller,
                signal_terminate: AccessLevel::Controller,
                signal_stop: AccessLevel::Controller,
                signal_continue: AccessLevel::Controller,
                signal_kill: AccessLevel::Owner,
                restart: AccessLevel::Owner,
                delete: AccessLevel::Owner,
                launch: AccessLevel::Owner,
            },
            state: JobState::Running,
            started_at: chrono::DateTime::UNIX_EPOCH,
            finished_at: None,
            terminal: crate::process::TerminalMetadata::default(),
            log: crate::job::LogMetadata::default(),
        }
    }

    #[test]
    fn anonymous_public_output_allowed() {
        let job = owner_record();
        assert_eq!(
            authorize(&Principal::Anonymous, &job, Permission::ViewStatus),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            authorize(&Principal::Anonymous, &job, Permission::ViewOutput),
            AuthorizationDecision::Allow
        );
    }

    #[test]
    fn anonymous_cannot_send_input() {
        let job = owner_record();
        assert!(matches!(
            authorize(&Principal::Anonymous, &job, Permission::SendInput),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn local_owner_can_do_everything() {
        let job = owner_record();
        let owner = Principal::LocalUnixUser { uid: 1000 };
        for p in [
            Permission::ViewStatus,
            Permission::ViewOutput,
            Permission::ViewCommand,
            Permission::ViewWorkingDirectory,
            Permission::SendInput,
            Permission::SendInterrupt,
            Permission::SendTerminate,
            Permission::SendStop,
            Permission::SendContinue,
            Permission::SendKill,
            Permission::Restart,
            Permission::Delete,
            Permission::DownloadLogs,
            Permission::ModifyPolicy,
            Permission::Launch,
        ] {
            assert_eq!(
                authorize(&owner, &job, p),
                AuthorizationDecision::Allow,
                "{p:?}"
            );
        }
    }

    #[test]
    fn other_user_is_not_owner() {
        let job = owner_record();
        let intruder = Principal::LocalUnixUser { uid: 2000 };
        assert!(matches!(
            authorize(&intruder, &job, Permission::SendKill),
            AuthorizationDecision::Deny { .. }
        ));
        // But the intruder can still see public output.
        assert_eq!(
            authorize(&intruder, &job, Permission::ViewOutput),
            AuthorizationDecision::Allow
        );
    }

    #[test]
    fn controller_can_interrupt_but_not_kill() {
        let job = owner_record();
        let controller = Principal::BrowserSession {
            session_id: "s".into(),
        };
        assert_eq!(
            authorize(&controller, &job, Permission::SendInterrupt),
            AuthorizationDecision::Allow
        );
        assert!(matches!(
            authorize(&controller, &job, Permission::SendKill),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn read_only_token_cannot_send_input() {
        let job = owner_record();
        let token = Principal::ApiToken {
            token_id: "t".into(),
            grants: vec![TokenGrant {
                resource: ResourceSelector::AllOwned,
                permissions: [Permission::ViewStatus, Permission::ViewOutput].into(),
                global: Default::default(),
            }],
        };
        assert_eq!(
            authorize(&token, &job, Permission::ViewOutput),
            AuthorizationDecision::Allow
        );
        assert!(matches!(
            authorize(&token, &job, Permission::SendInput),
            AuthorizationDecision::Deny { .. }
        ));
        // Command is authenticated, token is authenticated, but lacks the grant.
        assert!(matches!(
            authorize(&token, &job, Permission::ViewCommand),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn controller_token_with_grant_can_signal() {
        let job = owner_record();
        let token = Principal::ApiToken {
            token_id: "t".into(),
            grants: vec![TokenGrant {
                resource: ResourceSelector::AllOwned,
                permissions: [Permission::SendInterrupt, Permission::ViewOutput].into(),
                global: Default::default(),
            }],
        };
        assert_eq!(
            authorize(&token, &job, Permission::SendInterrupt),
            AuthorizationDecision::Allow
        );
        // But not SIGKILL even though a controller role would, because scopes
        // are evaluated independently.
        assert!(matches!(
            authorize(&token, &job, Permission::SendKill),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn job_restricted_token_matches_only_that_job() {
        let job = owner_record();
        let other_job_id = crate::JobId::from_str("01ARZ3NDEKTSV4RRFFQ69G5FAV").expect("parse");
        let token = Principal::ApiToken {
            token_id: "t".into(),
            grants: vec![TokenGrant {
                resource: ResourceSelector::Job(other_job_id),
                permissions: [Permission::SendKill].into(),
                global: Default::default(),
            }],
        };
        assert!(matches!(
            authorize(&token, &job, Permission::SendKill),
            AuthorizationDecision::Deny { .. }
        ));
    }

    fn global_token(perms: &[GlobalPermission]) -> Principal {
        Principal::ApiToken {
            token_id: "t".into(),
            grants: vec![TokenGrant {
                resource: ResourceSelector::AllOwned,
                permissions: Default::default(),
                global: perms.iter().copied().collect(),
            }],
        }
    }

    #[test]
    fn local_user_allowed_all_global_permissions() {
        let owner = Principal::LocalUnixUser { uid: 1000 };
        for p in [
            GlobalPermission::LaunchManagedProcess,
            GlobalPermission::LaunchInNewTerminal,
            GlobalPermission::LaunchInExistingTerminal,
            GlobalPermission::ProbeInterpreterHelp,
            GlobalPermission::ProbeExecutableHelp,
            GlobalPermission::ProbeScriptHelp,
            GlobalPermission::InspectStaticMetadata,
            GlobalPermission::InspectManPage,
        ] {
            assert_eq!(authorize_global(&owner, p), AuthorizationDecision::Allow);
        }
    }

    #[test]
    fn anonymous_cannot_launch_or_probe() {
        let anon = Principal::Anonymous;
        assert!(matches!(
            authorize_global(&anon, GlobalPermission::LaunchManagedProcess),
            AuthorizationDecision::Deny { .. }
        ));
        assert!(matches!(
            authorize_global(&anon, GlobalPermission::InspectManPage),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn token_global_scope_grants_only_listed_permissions() {
        let token = global_token(&[GlobalPermission::LaunchManagedProcess]);
        assert_eq!(
            authorize_global(&token, GlobalPermission::LaunchManagedProcess),
            AuthorizationDecision::Allow
        );
        // Not granted: opening a new terminal or probing.
        assert!(matches!(
            authorize_global(&token, GlobalPermission::LaunchInNewTerminal),
            AuthorizationDecision::Deny { .. }
        ));
        assert!(matches!(
            authorize_global(&token, GlobalPermission::ProbeScriptHelp),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn browser_session_controller_cannot_owner_only_ops() {
        let session = Principal::BrowserSession {
            session_id: "s".into(),
        };
        // Controller may launch managed processes and inspect man pages.
        assert_eq!(
            authorize_global(&session, GlobalPermission::LaunchManagedProcess),
            AuthorizationDecision::Allow
        );
        assert_eq!(
            authorize_global(&session, GlobalPermission::InspectManPage),
            AuthorizationDecision::Allow
        );
        // But not owner-only operations.
        assert!(matches!(
            authorize_global(&session, GlobalPermission::LaunchInExistingTerminal),
            AuthorizationDecision::Deny { .. }
        ));
        assert!(matches!(
            authorize_global(&session, GlobalPermission::ProbeScriptHelp),
            AuthorizationDecision::Deny { .. }
        ));
    }

    #[test]
    fn shell_execution_disabled_for_everyone() {
        for principal in [
            Principal::LocalUnixUser { uid: 1000 },
            Principal::BrowserSession {
                session_id: "s".into(),
            },
            global_token(&[GlobalPermission::LaunchManagedProcess]),
        ] {
            assert_eq!(
                authorize_global(&principal, GlobalPermission::UseShellExecution),
                AuthorizationDecision::Disabled
            );
        }
    }
}
