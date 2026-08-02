//! Token scope evaluation: translate stored scopes into core permissions and
//! grants.

use std::collections::BTreeSet;

use jobwrap_core::{Permission, ResourceSelector, TokenGrant};
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
        let (resource, perm_part) = match parts {
            [j, id, perm @ ..] if j == "job" && id == "*" => (ResourceSelector::AllOwned, perm),
            [j, id, perm @ ..] if j == "job" => {
                let Some(job_id) = id.parse().ok() else {
                    continue;
                };
                (ResourceSelector::Job(job_id), perm)
            }
            _ => continue,
        };
        let permissions: BTreeSet<Permission> = permission_for(perm_part).into_iter().collect();
        if permissions.is_empty() {
            continue;
        }
        grants.push(TokenGrant {
            resource,
            permissions,
        });
    }
    grants
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
    fn jobs_list_scope_grants_no_job_permissions() {
        let token = stored(&["jobs:list"]);
        assert!(token_to_grants(&token).is_empty());
    }

    #[test]
    fn invalid_scope_ignored() {
        let token = stored(&["job:*:signal:evil"]);
        assert!(token_to_grants(&token).is_empty());
    }
}
