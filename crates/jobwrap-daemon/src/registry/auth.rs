//! Authentication and token operations backed by the registry's store.

use chrono::{Duration, Utc};
use jobwrap_core::Principal;
use jobwrap_protocol::{TokenCreateRequest, TokenInfo};
use jobwrap_store::{parse_scope, StoredSession, StoredToken};

use super::Registry;

/// A window of password attempts for rate limiting.
fn attempt_window() -> Duration {
    Duration::seconds(60)
}

/// Authentication operations on the registry.
pub struct AuthOps<'a> {
    pub registry: &'a Registry,
}

impl<'a> AuthOps<'a> {
    /// The stored password hash, if any.
    pub fn password_hash(&self) -> Option<String> {
        self.registry
            .store
            .lock()
            .expect("store lock")
            .get_password_hash()
            .ok()
            .flatten()
    }

    pub fn set_password(&self, password: &str) -> Result<(), String> {
        let hash = jobwrap_auth::hash_password(password).map_err(|e| e.to_string())?;
        self.registry
            .store
            .lock()
            .expect("store lock")
            .set_password_hash(&hash)
            .map_err(|e| e.to_string())
    }

    pub fn remove_password(&self) -> Result<(), String> {
        self.registry
            .store
            .lock()
            .expect("store lock")
            .remove_password()
            .map_err(|e| e.to_string())
    }

    pub fn password_set(&self) -> bool {
        self.password_hash().is_some()
    }

    /// Attempt a password login; on success returns a raw session token.
    pub fn login(&self, password: &str) -> Result<String, jobwrap_web::ApiError> {
        let config = &self.registry.config.authentication;
        if !self.password_set() {
            return Err(jobwrap_web::ApiError::unauthorized(
                "no password is configured",
            ));
        }
        // Rate limit password attempts.
        {
            let mut attempts = self.registry.login_attempts.lock().expect("attempts lock");
            let now = Utc::now();
            attempts.retain(|t| now.signed_duration_since(*t) < attempt_window());
            if attempts.len() >= config.password_attempt_limit as usize {
                return Err(jobwrap_web::ApiError::new(
                    jobwrap_protocol::ApiErrorCode::RateLimited,
                    "too many failed attempts; try again shortly",
                ));
            }
            let stored = self.password_hash().unwrap_or_default();
            if !jobwrap_auth::verify_password(password, &stored).unwrap_or(false) {
                attempts.push(now);
                return Err(jobwrap_web::ApiError::unauthorized("incorrect password"));
            }
        }
        // Create a session.
        let raw = jobwrap_auth::new_session_token()
            .map_err(|e| jobwrap_web::ApiError::internal(e.to_string()))?;
        let hash = jobwrap_auth::session_hash(&raw);
        let session = StoredSession {
            id: hash.clone(),
            hash,
            created_at: Utc::now(),
            expires_at: Utc::now() + Duration::minutes(config.browser_session_minutes as i64),
            user_label: "browser".to_string(),
        };
        self.registry
            .store
            .lock()
            .expect("store lock")
            .insert_session(&session)
            .map_err(|e| jobwrap_web::ApiError::internal(e.to_string()))?;
        Ok(raw)
    }

    pub fn logout(&self, session_token: &str) {
        let hash = jobwrap_auth::session_hash(session_token);
        let _ = self
            .registry
            .store
            .lock()
            .expect("store lock")
            .delete_session(&hash);
    }

    /// Resolve request credentials into a principal.
    pub fn resolve_principal(&self, session: Option<&str>, bearer: Option<&str>) -> Principal {
        if let Some(token) = bearer {
            let hash = jobwrap_auth::hash_token(token);
            let store = self.registry.store.lock().expect("store lock");
            if let Ok(Some(stored)) = store.get_token_by_hash(&hash) {
                if !stored.revoked && !token_expired(&stored) {
                    let grants = jobwrap_auth::token_to_grants(&stored);
                    let id = stored.id.clone();
                    let _ = store.touch_token_last_used(&stored.id);
                    return Principal::ApiToken {
                        token_id: id,
                        grants,
                    };
                }
            }
            return Principal::Anonymous;
        }
        if let Some(session_token) = session {
            let hash = jobwrap_auth::session_hash(session_token);
            let store = self.registry.store.lock().expect("store lock");
            if let Ok(Some(stored)) = store.get_session_by_hash(&hash) {
                if Utc::now() < stored.expires_at {
                    return Principal::BrowserSession {
                        session_id: stored.id,
                    };
                }
            }
            return Principal::Anonymous;
        }
        Principal::Anonymous
    }

    // ---- tokens ----

    pub fn create_token(&self, req: &TokenCreateRequest) -> Result<(String, String), String> {
        if req.name.is_empty() {
            return Err("token name is required".to_string());
        }
        let mut scopes = Vec::new();
        for scope in &req.scopes {
            let parsed = parse_scope(scope).ok_or_else(|| format!("invalid scope `{scope}`"))?;
            scopes.push(parsed);
        }
        if scopes.is_empty() {
            return Err("at least one valid scope is required".to_string());
        }
        let raw = jobwrap_auth::new_api_token().map_err(|e| e.to_string())?;
        let hash = jobwrap_auth::hash_token(&raw);
        let expires_at = req
            .expires_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        let id = jobwrap_core::JobId::generate().map_err(|e| e.to_string())?;
        let token = StoredToken {
            id: id.to_string(),
            name: req.name.clone(),
            hash,
            scopes,
            job_id: req.job_id,
            created_at: Utc::now(),
            expires_at,
            last_used_at: None,
            revoked: false,
        };
        self.registry
            .store
            .lock()
            .expect("store lock")
            .insert_token(&token)
            .map_err(|e| e.to_string())?;
        Ok((id.to_string(), raw))
    }

    pub fn list_tokens(&self) -> Vec<TokenInfo> {
        self.registry
            .store
            .lock()
            .expect("store lock")
            .list_tokens()
            .unwrap_or_default()
            .into_iter()
            .map(|t| TokenInfo {
                id: t.id,
                name: t.name,
                scopes: t.scopes.iter().map(|s| s.raw.clone()).collect(),
                job_id: t.job_id,
                created_at: t.created_at,
                expires_at: t.expires_at,
                last_used_at: t.last_used_at,
                revoked: t.revoked,
            })
            .collect()
    }

    pub fn revoke_token(&self, token_id: &str) -> Result<(), String> {
        self.registry
            .store
            .lock()
            .expect("store lock")
            .revoke_token(token_id)
            .map_err(|e| e.to_string())
    }
}

fn token_expired(token: &StoredToken) -> bool {
    token.expires_at.map(|t| Utc::now() >= t).unwrap_or(false)
}
