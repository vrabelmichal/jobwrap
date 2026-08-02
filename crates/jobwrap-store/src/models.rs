//! Stored authentication and token models.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use jobwrap_core::JobId;

/// A parsed token scope such as `jobs:list`, `job:*:output`, or
/// `job:<id>:signal:int`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenScope {
    /// The scope string as provided by the user.
    pub raw: String,
    /// Parsed permission tokens (see [`parse_scope`]).
    pub parsed: Vec<String>,
}

/// Parse a human scope string into structured tokens.
///
/// Accepts forms like `jobs:list`, `job:*:output`, `job:<ulid>:signal:int`.
pub fn parse_scope(raw: &str) -> Option<TokenScope> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let parts: Vec<&str> = raw.split(':').collect();
    let valid = match parts.as_slice() {
        ["jobs", "list"] | ["jobs", "status"] | ["jobs", "output"] => true,
        ["job", "*", "status"] | ["job", "*", "output"] | ["job", "*", "input"] => true,
        ["job", id, "status"] | ["job", id, "output"] | ["job", id, "input"] => {
            id.parse::<JobId>().is_ok()
        }
        ["job", "*", "signal", sig] => matches!(*sig, "int" | "term" | "stop" | "cont" | "kill"),
        ["job", id, "signal", sig] => {
            id.parse::<JobId>().is_ok() && matches!(*sig, "int" | "term" | "stop" | "cont" | "kill")
        }
        ["server", "status"] => true,
        _ => false,
    };
    if valid {
        Some(TokenScope {
            raw: raw.to_string(),
            parsed: parts.iter().map(|s| s.to_string()).collect(),
        })
    } else {
        None
    }
}

/// A stored API token (only the hash and metadata are persisted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredToken {
    pub id: String,
    pub name: String,
    /// SHA-256 hex of the token value.
    pub hash: String,
    pub scopes: Vec<TokenScope>,
    pub job_id: Option<JobId>,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked: bool,
}

/// A stored browser session (only the hash of the opaque session id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredSession {
    pub id: String,
    /// SHA-256 hex of the opaque session token.
    pub hash: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub user_label: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_scopes() {
        for raw in [
            "jobs:list",
            "jobs:status",
            "job:*:output",
            "job:01ARZ3NDEKTSV4RRFFQ69G5FAV:signal:int",
            "server:status",
        ] {
            assert!(parse_scope(raw).is_some(), "{raw}");
        }
    }

    #[test]
    fn rejects_invalid_scopes() {
        for raw in [
            "",
            "jobs",
            "job:*:kill_extra",
            "job:notaulid:output",
            "jobs:signal:term",
        ] {
            assert!(parse_scope(raw).is_none(), "{raw}");
        }
    }
}
