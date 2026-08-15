use anyhow::{Context, Result};
use rusqlite::Connection;

/// Schema DDL, executed idempotently on every open.
///
/// `started_at` is stored as RFC3339 UTC text so time-window queries become
/// lexicographic range scans (`>= ?1 AND < ?2`). Dedup is a UNIQUE fingerprint
/// plus `INSERT OR IGNORE`.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS usage_records (
    id                 INTEGER PRIMARY KEY AUTOINCREMENT,
    provider           TEXT    NOT NULL,                -- 'claude' | 'codex' | ...
    project            TEXT    NOT NULL,
    session_id         TEXT    NOT NULL DEFAULT '',
    model              TEXT    NOT NULL DEFAULT '',
    started_at         TEXT    NOT NULL,                -- RFC3339 UTC
    input_tokens       INTEGER NOT NULL DEFAULT 0,
    output_tokens      INTEGER NOT NULL DEFAULT 0,
    cache_read_tokens  INTEGER NOT NULL DEFAULT 0,
    cache_write_tokens INTEGER NOT NULL DEFAULT 0,
    cost_micros        INTEGER NOT NULL DEFAULT 0,      -- USD * 1_000_000
    raw_bytes          INTEGER NOT NULL DEFAULT 0,
    fingerprint        TEXT    NOT NULL UNIQUE,         -- dedup key
    created_at         TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now'))
);
CREATE INDEX IF NOT EXISTS idx_usage_provider_started ON usage_records(provider, started_at);
CREATE INDEX IF NOT EXISTS idx_usage_project_started  ON usage_records(project,  started_at);
CREATE INDEX IF NOT EXISTS idx_usage_model            ON usage_records(model);

CREATE TABLE IF NOT EXISTS projects (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT NOT NULL UNIQUE,
    path      TEXT NOT NULL DEFAULT '',
    providers TEXT NOT NULL DEFAULT '[]'               -- JSON array of provider ids
);

CREATE TABLE IF NOT EXISTS quotas (
    provider          TEXT NOT NULL,
    period            TEXT NOT NULL,                   -- 'day' | 'week' | 'month'
    limit_tokens      INTEGER NOT NULL DEFAULT 0,      -- 0 = unlimited
    used_tokens       INTEGER NOT NULL DEFAULT 0,
    limit_cost_micros INTEGER NOT NULL DEFAULT 0,
    used_cost_micros  INTEGER NOT NULL DEFAULT 0,
    updated_at        TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ','now')),
    PRIMARY KEY (provider, period)
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// Open (or create) the database at `db_path`, apply pragmas, and init the schema.
pub fn open(db_path: &std::path::Path) -> Result<Connection> {
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent).context("create db parent dir")?;
    }
    let conn = Connection::open(db_path).context("open sqlite database")?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    init_schema(&conn)?;
    Ok(conn)
}

/// Open a read-only connection for UI aggregation.
///
/// Does NOT run `init_schema` (which would briefly take a write lock and
/// contend with the scan writer). WAL journal mode is persisted in the DB file,
/// so a reader opened here sees a consistent snapshot without ever blocking the
/// writer.
pub fn open_read(db_path: &std::path::Path) -> Result<Connection> {
    let conn = Connection::open(db_path).context("open sqlite database (read)")?;
    conn.pragma_update(None, "query_only", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    Ok(conn)
}

/// Run the schema DDL. Safe to call repeatedly.
pub fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA_SQL).context("init schema")
}
