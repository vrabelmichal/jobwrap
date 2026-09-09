//! Token scope evaluation: translate stored scopes into core permissions and
//! grants.

use std::collections::BTreeSet;

use jobwrap_core::{GlobalPermission, Permission, ResourceSelector, TokenGrant};
use jobwrap_store::StoredToken;

/// Map a scope permission segment to the core permissions it grants.
fn permission_for(scope: &[String]) -> Vec<Permission> {
    match scope {
        [p] if p.as_str() == "status" => vec![Permission::ViewStatus],
        [p] if p.as_str() == "output" => vec![Permission::ViewOutput, Permission::DownloadLogs],
        [p] if p.as_str() == "command" => vec![Permission::ViewCommand],
        [p] if p.as_str() == "input" => vec![Permission::SendInput],
        [p, sig] if p.as_str() == "signal" => match sig.as_str() {
            "int" => vec![Permission::SendInterrupt],
            "term" => vec![Permission::SendTerminate],
            "stop" => vec![Permission::SendStop],
            "cont" => vec![Permission::SendContinue],
            "kill" => vec![Permission::SendKill],
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Compute the set of permissions this token holds across all its scopes.
pub fn token_to_permissions(token: &StoredToken) -> BTreeSet<Permission> {
    let mut perms = BTreeSet::new();
    for scope in &token.scopes {
        let parts = scope.parsed.as_slice();
        if let [j, _, perm @ ..] = parts {
            if j == "job" {
                for p in permission_for(perm) {
                    perms.insert(p);
                }
            }
        }
    }
    perms
}

/// Translate a stored token into core [`TokenGrant`]s for authorization.
pub fn token_to_grants(token: &StoredToken) -> Vec<TokenGrant> {
    let mut grants: Vec<TokenGrant> = Vec::new();
    for scope in &token.scopes {
        let parts = scope.parsed.as_slice();
        let grant = match parts {
            [j, id, perm @ ..] if j == "job" && id == "*" => {
                let permissions: BTreeSet<Permission> = permission_for(perm).into_iter().collect();
                if permissions.is_empty() {
                    continue;
                }
                Some(TokenGrant {
                    resource: ResourceSelector::AllOwned,
                    permissions,
                    global: BTreeSet::new(),
                })
            }
            [j, id, perm @ ..] if j == "job" => {
                let Some(job_id) = id.parse().ok() else {
                    continue;
                };
                let permissions: BTreeSet<Permission> = permission_for(perm).into_iter().collect();
                if permissions.is_empty() {
                    continue;
                }
                Some(TokenGrant {
                    resource: ResourceSelector::Job(job_id),
                    permissions,
                    global: BTreeSet::new(),
                })
            }
            [j, action] if j == "jobs" => {
                let global: BTreeSet<GlobalPermission> =
                    std::iter::once(global_action_permission(action))
                        .flatten()
                        .collect();
                if global.is_empty() {
                    continue;
                }
                Some(TokenGrant {
                    resource: ResourceSelector::AllOwned,
                    permissions: BTreeSet::new(),
                    global,
                })
            }
            [j, action, target] if j == "jobs" && action == "launch" => {
                let global: BTreeSet<GlobalPermission> = match target.as_str() {
                    "new-terminal" => [GlobalPermission::LaunchInNewTerminal]
                        .into_iter()
                        .collect(),
                    "existing-terminal" => [GlobalPermission::LaunchInExistingTerminal]
                        .into_iter()
                        .collect(),
                    _ => continue,
                };
                Some(TokenGrant {
                    resource: ResourceSelector::AllOwned,
                    permissions: BTreeSet::new(),
                    global,
                })
            }
            [j, kind, action] if j == "docs" => {
                let global: BTreeSet<GlobalPermission> =
                    std::iter::once(docs_action_permission(kind, action))
                        .flatten()
                        .collect();
                if global.is_empty() {
                    continue;
                }
                Some(TokenGrant {
                    resource: ResourceSelector::AllOwned,
                    permissions: BTreeSet::new(),
                    global,
                })
            }
            _ => continue,
        };
        if let Some(g) = grant {
            grants.push(g);
        }
    }
    grants
}

/// Map a global `jobs:*` action to a core [`GlobalPermission`], if any.
fn global_action_permission(action: &str) -> Option<GlobalPermission> {
    match action {
        "launch" => Some(GlobalPermission::LaunchManagedProcess),
        "launch:new-terminal" => Some(GlobalPermission::LaunchInNewTerminal),
        "launch:existing-terminal" => Some(GlobalPermission::LaunchInExistingTerminal),
        _ => None,
    }
}

/// Map a `docs:<kind>:<action>` scope to a core [`GlobalPermission`].
fn docs_action_permission(kind: &str, action: &str) -> Option<GlobalPermission> {
    match (kind, action) {
        ("identify", _) => Some(GlobalPermission::InspectStaticMetadata),
        ("man", _) => Some(GlobalPermission::InspectManPage),
        ("probe", "interpreter") => Some(GlobalPermission::ProbeInterpreterHelp),
        ("probe", "executable") => Some(GlobalPermission::ProbeExecutableHelp),
        ("probe", "script") => Some(GlobalPermission::ProbeScriptHelp),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobwrap_store::TokenScope;

    fn stored(scopes: &[&str]) -> StoredToken {
        StoredToken {
            id: "t".into(),
            name: "agent".into(),
            hash: "h".into(),
            scopes: scopes
                .iter()
                .map(|s| {
                    let parsed: Vec<String> = s.split(':').map(str::to_string).collect();
                    TokenScope {
                        raw: s.to_string(),
                        parsed,
                    }
                })
                .collect(),
            job_id: None,
            created_at: chrono::Utc::now(),
            expires_at: None,
            last_used_at: None,
            revoked: false,
        }
    }

    #[test]
    fn wildcard_output_grant() {
        let token = stored(&["job:*:output"]);
        let grants = token_to_grants(&token);
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].resource, ResourceSelector::AllOwned);
        assert!(grants[0].permissions.contains(&Permission::ViewOutput));
        assert!(grants[0].permissions.contains(&Permission::DownloadLogs));
    }

    #[test]
    fn specific_signal_grant() {
        let id = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
        let token = stored(&[&format!("job:{id}:signal:int")]);
        let grants = token_to_grants(&token);
        assert_eq!(grants.len(), 1);
        assert!(matches!(grants[0].resource, ResourceSelector::Job(j) if j.to_string() == id));
        assert!(grants[0].permissions.contains(&Permission::SendInterrupt));
        assert!(!grants[0].permissions.contains(&Permission::SendKill));
    }

    #[test]
    fn launch_scope_grants_global_permission() {
        let token = stored(&["jobs:launch"]);
        let grants = token_to_grants(&token);
        assert_eq!(grants.len(), 1);
        assert!(grants[0]
            .global
            .contains(&GlobalPermission::LaunchManagedProcess));
        assert!(grants[0].permissions.is_empty());
    }

    #[test]
    fn docs_man_scope() {
        let token = stored(&["docs:man:all"]);
        let grants = token_to_grants(&token);
        assert_eq!(grants.len(), 1);
        assert!(grants[0].global.contains(&GlobalPermission::InspectManPage));
    }

    #[test]
    fn jobs_list_scope_grants_no_permissions() {
        let token = stored(&["jobs:list"]);
        assert!(token_to_grants(&token).is_empty());
    }

    #[test]
    fn invalid_scope_ignored() {
        let token = stored(&["job:*:signal:evil"]);
        assert!(token_to_grants(&token).is_empty());
    }
}
