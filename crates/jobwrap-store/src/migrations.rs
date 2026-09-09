//! SQLite schema migrations.
//!
//! The schema version is tracked in `PRAGMA user_version`. Only forward
//! migrations are allowed.

use rusqlite::Connection;

pub const CURRENT_VERSION: i32 = 1;

/// Apply any pending migrations.
pub fn migrate(conn: &mut Connection) -> rusqlite::Result<()> {
    let current: i32 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    if current > CURRENT_VERSION {
        return Err(rusqlite::Error::QueryReturnedNoRows); // should not happen
    }
    if current < 1 {
        migration_001(conn)?;
        conn.pragma_update(None, "user_version", 1)?;
    }
    Ok(())
}

fn migration_001(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS jobs (
            id            TEXT PRIMARY KEY,
            record_json   TEXT NOT NULL,
            owner_uid     INTEGER NOT NULL,
            state         TEXT NOT NULL,
            profile_name  TEXT NOT NULL,
            display_name  TEXT NOT NULL,
            started_at    TEXT NOT NULL,
            finished_at   TEXT,
            last_sequence INTEGER NOT NULL DEFAULT 0,
            output_bytes  INTEGER NOT NULL DEFAULT 0,
            log_truncated INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS jobs_owner_idx ON jobs (owner_uid);
        CREATE INDEX IF NOT EXISTS jobs_state_idx ON jobs (state);

        CREATE TABLE IF NOT EXISTS events (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id    TEXT NOT NULL,
            event_kind TEXT NOT NULL,
            at        TEXT NOT NULL,
            detail    TEXT
        );
        CREATE INDEX IF NOT EXISTS events_job_idx ON events (job_id, id);

        CREATE TABLE IF NOT EXISTS tokens (
            id           TEXT PRIMARY KEY,
            name         TEXT NOT NULL,
            hash         TEXT NOT NULL UNIQUE,
            scopes_json  TEXT NOT NULL,
            job_id       TEXT,
            created_at   TEXT NOT NULL,
            expires_at   TEXT,
            last_used_at TEXT,
            revoked      INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id         TEXT PRIMARY KEY,
            hash       TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            expires_at TEXT NOT NULL,
            user_label TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS auth (
            id           INTEGER PRIMARY KEY CHECK (id = 1),
            password_hash TEXT
        );

        CREATE TABLE IF NOT EXISTS audit (
            id        INTEGER PRIMARY KEY AUTOINCREMENT,
            job_id    TEXT,
            at        TEXT NOT NULL,
            actor     TEXT NOT NULL,
            operation TEXT NOT NULL,
            detail    TEXT
        );

        CREATE TABLE IF NOT EXISTS help_cache (
            key               TEXT PRIMARY KEY,
            target_path       TEXT NOT NULL,
            target_fingerprint TEXT NOT NULL,
            interpreter       TEXT,
            probe_argument    TEXT NOT NULL,
            classification    TEXT NOT NULL,
            output_digest     TEXT NOT NULL,
            warning           TEXT,
            cached_at         TEXT NOT NULL
        );
        "#,
    )
}
