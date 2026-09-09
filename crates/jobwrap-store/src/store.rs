//! SQLite-backed repositories.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use thiserror::Error;

use jobwrap_core::{Event, EventId, EventKind, JobId, JobRecord};

use super::migrations;
use super::models::{StoredSession, StoredToken, TokenScope};

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not serialize record: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("corrupt record for job {job_id}: {detail}")]
    Corrupt { job_id: String, detail: String },
}

/// The SQLite store.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

fn now_rfc() -> String {
    Utc::now().to_rfc3339()
}

fn parse_rfc(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

impl Store {
    /// Open (creating if needed) the database and apply migrations.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            let metadata = std::fs::symlink_metadata(parent)?;
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(StoreError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "database directory must be a real directory, not a symlink",
                )));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        reject_unsafe_database_path(path)?;
        reject_unsafe_database_path(&sidecar_path(path, "-wal"))?;
        reject_unsafe_database_path(&sidecar_path(path, "-shm"))?;
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrations::migrate(&mut conn)?;
        Ok(Self { conn })
    }

    // ---- jobs ----

    pub fn insert_job(&self, record: &JobRecord) -> Result<(), StoreError> {
        let json = serde_json::to_string(record)?;
        self.conn.execute(
            "INSERT INTO jobs (id, record_json, owner_uid, state, profile_name, display_name, started_at, finished_at, last_sequence, output_bytes, log_truncated)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
             ON CONFLICT(id) DO UPDATE SET
               record_json=excluded.record_json,
               owner_uid=excluded.owner_uid,
               state=excluded.state,
               profile_name=excluded.profile_name,
               display_name=excluded.display_name,
               started_at=excluded.started_at,
               finished_at=excluded.finished_at,
               last_sequence=excluded.last_sequence,
               output_bytes=excluded.output_bytes,
               log_truncated=excluded.log_truncated",
            params![
                record.id.to_string(),
                json,
                record.owner_uid,
                record.state.label(),
                record.profile_name,
                record.display_name.as_str(),
                record.started_at.to_rfc3339(),
                record.finished_at.map(|t| t.to_rfc3339()),
                record.log.last_sequence,
                record.log.bytes_written,
                if record.log.truncated { 1 } else { 0 },
            ],
        )?;
        Ok(())
    }

    pub fn update_job(&self, record: &JobRecord) -> Result<(), StoreError> {
        self.insert_job(record)
    }

    pub fn get_job(&self, id: JobId) -> Result<Option<JobRecord>, StoreError> {
        let row: Option<String> = self
            .conn
            .query_row(
                "SELECT record_json FROM jobs WHERE id = ?1",
                params![id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        match row {
            Some(json) => {
                let record = serde_json::from_str(&json).map_err(|e| StoreError::Corrupt {
                    job_id: id.to_string(),
                    detail: e.to_string(),
                })?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    pub fn list_jobs(&self) -> Result<Vec<JobRecord>, StoreError> {
        let mut stmt = self
            .conn
            .prepare("SELECT record_json FROM jobs ORDER BY started_at DESC LIMIT 10000")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            let json = row?;
            let record: JobRecord =
                serde_json::from_str(&json).map_err(|e| StoreError::Corrupt {
                    job_id: "?".into(),
                    detail: e.to_string(),
                })?;
            out.push(record);
        }
        Ok(out)
    }

    pub fn delete_job(&mut self, id: JobId) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM jobs WHERE id = ?1", params![id.to_string()])?;
        Ok(())
    }

    // ---- events ----

    pub fn insert_event(&self, event: &Event) -> Result<(), StoreError> {
        let detail = serde_json::to_string(&event.kind)?;
        self.conn.execute(
            "INSERT INTO events (job_id, event_kind, at, detail) VALUES (?1, ?2, ?3, ?4)",
            params![
                event.job_id.to_string(),
                event.kind.label(),
                event.at.to_rfc3339(),
                detail
            ],
        )?;
        Ok(())
    }

    pub fn events_for_job(&self, id: JobId) -> Result<Vec<Event>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, at, detail FROM events WHERE job_id = ?1 ORDER BY id ASC LIMIT 1000",
        )?;
        let rows = stmt.query_map(params![id.to_string()], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (seq, at, detail) = row?;
            let kind: EventKind = serde_json::from_str(&detail).unwrap_or(EventKind::Audit {
                detail: detail.clone(),
            });
            out.push(Event {
                id: EventId(seq as u64),
                job_id: id,
                at: parse_rfc(&at).unwrap_or_else(Utc::now),
                kind,
            });
        }
        Ok(out)
    }

    // ---- auth ----

    pub fn get_password_hash(&self) -> Result<Option<String>, StoreError> {
        self.conn
            .query_row("SELECT password_hash FROM auth WHERE id = 1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(StoreError::from)
    }

    pub fn set_password_hash(&self, hash: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO auth (id, password_hash) VALUES (1, ?1)
             ON CONFLICT(id) DO UPDATE SET password_hash = excluded.password_hash",
            params![hash],
        )?;
        Ok(())
    }

    pub fn remove_password(&self) -> Result<(), StoreError> {
        self.conn
            .execute("UPDATE auth SET password_hash = NULL WHERE id = 1", [])?;
        Ok(())
    }

    // ---- tokens ----

    pub fn insert_token(&self, token: &StoredToken) -> Result<(), StoreError> {
        let scopes = serde_json::to_string(&token.scopes)?;
        self.conn.execute(
            "INSERT INTO tokens (id, name, hash, scopes_json, job_id, created_at, expires_at, last_used_at, revoked)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                token.id,
                token.name,
                token.hash,
                scopes,
                token.job_id.map(|j| j.to_string()),
                token.created_at.to_rfc3339(),
                token.expires_at.map(|t| t.to_rfc3339()),
                token.last_used_at.map(|t| t.to_rfc3339()),
                if token.revoked { 1 } else { 0 },
            ],
        )?;
        Ok(())
    }

    pub fn get_token_by_hash(&self, hash: &str) -> Result<Option<StoredToken>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, hash, scopes_json, job_id, created_at, expires_at, last_used_at, revoked
             FROM tokens WHERE hash = ?1",
        )?;
        let mut rows = stmt.query_map(params![hash], Self::map_token)?;
        rows.next().transpose().map_err(StoreError::from)
    }

    pub fn list_tokens(&self) -> Result<Vec<StoredToken>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, hash, scopes_json, job_id, created_at, expires_at, last_used_at, revoked
             FROM tokens ORDER BY created_at DESC",
        )?;
        let rows = stmt.query_map([], Self::map_token)?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(StoreError::from)
    }

    fn map_token(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredToken> {
        let scopes_json: String = row.get(3)?;
        let scopes: Vec<TokenScope> = serde_json::from_str(&scopes_json).unwrap_or_default();
        let job_id: Option<String> = row.get(4)?;
        let expires_at: Option<String> = row.get(6)?;
        let last_used_at: Option<String> = row.get(7)?;
        Ok(StoredToken {
            id: row.get(0)?,
            name: row.get(1)?,
            hash: row.get(2)?,
            scopes,
            job_id: job_id.and_then(|s| s.parse().ok()),
            created_at: parse_rfc(&row.get::<_, String>(5)?).unwrap_or_else(Utc::now),
            expires_at: expires_at.as_deref().and_then(parse_rfc),
            last_used_at: last_used_at.as_deref().and_then(parse_rfc),
            revoked: row.get::<_, i64>(8)? != 0,
        })
    }

    pub fn revoke_token(&self, id: &str) -> Result<(), StoreError> {
        self.conn
            .execute("UPDATE tokens SET revoked = 1 WHERE id = ?1", params![id])?;
        Ok(())
    }

    pub fn touch_token_last_used(&self, id: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE tokens SET last_used_at = ?1 WHERE id = ?2",
            params![now_rfc(), id],
        )?;
        Ok(())
    }

    // ---- sessions ----

    pub fn insert_session(&self, session: &StoredSession) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO sessions (id, hash, created_at, expires_at, user_label)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                session.id,
                session.hash,
                session.created_at.to_rfc3339(),
                session.expires_at.to_rfc3339(),
                session.user_label,
            ],
        )?;
        Ok(())
    }

    pub fn get_session_by_hash(&self, hash: &str) -> Result<Option<StoredSession>, StoreError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, hash, created_at, expires_at, user_label FROM sessions WHERE hash = ?1",
        )?;
        let mut rows = stmt.query_map(params![hash], |row| {
            Ok(StoredSession {
                id: row.get(0)?,
                hash: row.get(1)?,
                created_at: parse_rfc(&row.get::<_, String>(2)?).unwrap_or_else(Utc::now),
                expires_at: parse_rfc(&row.get::<_, String>(3)?).unwrap_or_else(Utc::now),
                user_label: row.get(4)?,
            })
        })?;
        rows.next().transpose().map_err(StoreError::from)
    }

    pub fn delete_session(&self, hash: &str) -> Result<(), StoreError> {
        self.conn
            .execute("DELETE FROM sessions WHERE hash = ?1", params![hash])?;
        Ok(())
    }

    pub fn prune_expired_sessions(&self, now: DateTime<Utc>) -> Result<usize, StoreError> {
        self.conn
            .execute(
                "DELETE FROM sessions WHERE expires_at <= ?1",
                params![now.to_rfc3339()],
            )
            .map_err(StoreError::from)
    }

    pub fn session_count(&self) -> Result<u64, StoreError> {
        self.conn
            .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
            .map_err(StoreError::from)
    }

    // ---- audit ----

    pub fn record_audit(
        &self,
        job_id: Option<JobId>,
        actor: &str,
        operation: &str,
        detail: &str,
    ) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO audit (job_id, at, actor, operation, detail) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                job_id.map(|j| j.to_string()),
                now_rfc(),
                actor,
                operation,
                detail
            ],
        )?;
        Ok(())
    }

    // ---- help cache ----

    pub fn help_cache_get(&self, key: &str) -> Result<Option<HelpCacheEntry>, StoreError> {
        self.conn
            .query_row(
                "SELECT key, target_path, target_fingerprint, interpreter, probe_argument,
                        classification, output_digest, warning, cached_at
                 FROM help_cache WHERE key = ?1",
                params![key],
                |row| {
                    Ok(HelpCacheEntry {
                        key: row.get(0)?,
                        target_path: row.get(1)?,
                        target_fingerprint: row.get(2)?,
                        interpreter: row.get(3)?,
                        probe_argument: row.get(4)?,
                        classification: row.get(5)?,
                        output_digest: row.get(6)?,
                        warning: row.get(7)?,
                        cached_at: parse_rfc(&row.get::<_, String>(8)?).unwrap_or_else(Utc::now),
                    })
                },
            )
            .optional()
            .map_err(StoreError::from)
    }

    pub fn help_cache_put(&self, entry: &HelpCacheEntry) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO help_cache (key, target_path, target_fingerprint, interpreter,
                probe_argument, classification, output_digest, warning, cached_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(key) DO UPDATE SET
               target_path=excluded.target_path,
               target_fingerprint=excluded.target_fingerprint,
               interpreter=excluded.interpreter,
               probe_argument=excluded.probe_argument,
               classification=excluded.classification,
               output_digest=excluded.output_digest,
               warning=excluded.warning,
               cached_at=excluded.cached_at",
            params![
                entry.key,
                entry.target_path,
                entry.target_fingerprint,
                entry.interpreter,
                entry.probe_argument,
                entry.classification,
                entry.output_digest,
                entry.warning,
                entry.cached_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }
}

fn sidecar_path(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    value.into()
}

fn reject_unsafe_database_path(path: &Path) -> Result<(), StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("refusing unsafe database path {}", path.display()),
            )))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(StoreError::Io(error)),
    }
}

/// A cached help-probe result.
#[derive(Debug, Clone)]
pub struct HelpCacheEntry {
    pub key: String,
    pub target_path: String,
    pub target_fingerprint: String,
    pub interpreter: Option<String>,
    pub probe_argument: String,
    pub classification: String,
    pub output_digest: String,
    pub warning: Option<String>,
    pub cached_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use jobwrap_core::{AccessLevel, JobName, JobState, ProcessId, ProfileAccess};

    fn temp_store() -> (TempDir, Store) {
        let dir = TempDir::new();
        let store = Store::open(&dir.path.join("test.db")).expect("open");
        (dir, store)
    }

    fn sample_record() -> JobRecord {
        JobRecord {
            id: JobId::generate().expect("id"),
            display_name: JobName::new("test").expect("name"),
            owner_uid: 1000,
            wrapper_pid: Some(ProcessId(123)),
            child_pid: None,
            process_group_id: None,
            command: jobwrap_core::CommandDisplay::new(["python3".to_string()]).expect("cmd"),
            executable: std::path::PathBuf::from("/usr/bin/python3"),
            arguments: Vec::new(),
            working_directory: std::path::PathBuf::from("/tmp"),
            profile_name: "standard".to_string(),
            access_policy: ProfileAccess {
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
            }
            .into_policy(),
            state: JobState::Running,
            started_at: Utc::now(),
            finished_at: None,
            terminal: Default::default(),
            log: Default::default(),
        }
    }

    #[test]
    fn job_round_trip() {
        let (_dir, store) = temp_store();
        let record = sample_record();
        store.insert_job(&record).expect("insert");
        let got = store.get_job(record.id).expect("get").expect("exists");
        assert_eq!(got.id, record.id);
        assert_eq!(got.state, record.state);
        let jobs = store.list_jobs().expect("list");
        assert_eq!(jobs.len(), 1);
    }

    #[test]
    fn token_round_trip() {
        let (_dir, store) = temp_store();
        let token = StoredToken {
            id: "t1".into(),
            name: "agent".into(),
            hash: "abc123".into(),
            scopes: vec![TokenScope {
                raw: "jobs:list".into(),
                parsed: vec!["jobs".into(), "list".into()],
            }],
            job_id: None,
            created_at: Utc::now(),
            expires_at: None,
            last_used_at: None,
            revoked: false,
        };
        store.insert_token(&token).expect("insert");
        let got = store
            .get_token_by_hash("abc123")
            .expect("get")
            .expect("exists");
        assert_eq!(got.id, "t1");
        assert_eq!(got.scopes[0].raw, "jobs:list");
    }
}

/// A tiny temp-directory helper to avoid extra dev dependencies.
#[cfg(test)]
struct TempDir {
    path: std::path::PathBuf,
}

#[cfg(test)]
impl TempDir {
    fn new() -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let path =
            std::env::temp_dir().join(format!("jobwrap-store-test-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir { path }
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}
