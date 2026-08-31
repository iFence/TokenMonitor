//! Qoder CN data source: reads the app's local SQLite usage store.
//!
//! The store is auto-detected per platform:
//!   Windows `%APPDATA%\QoderCN\SharedClientCache\cache\db\local.db`
//!   macOS   `~/Library/Application Support/QoderCN/SharedClientCache/cache/db/local.db`
//!   Linux   `~/.config/QoderCN/SharedClientCache/cache/db/local.db`
//! and can be overridden with `TOKEN_MONITOR_QODER_CN_DB_PATH`.
//!
//! Parse strategy (best-effort, mirrors the OpenCode adapter): usage is read
//! from a `message` table's `data` JSON (`role`/`tokens`/`modelID`/`time`). If
//! the schema differs, the adapter emits nothing rather than failing.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::source::{fingerprint, ProviderConfig, ProviderError, ProviderSource, ScanOutput};

/// Environment variable override for the Qoder CN SQLite path.
const QODER_DB_OVERRIDE: &str = "TOKEN_MONITOR_QODER_CN_DB_PATH";

/// Minimal view of a `message` row's `data` JSON (same shape as OpenCode).
#[derive(Deserialize)]
struct MessageData {
    role: Option<String>,
    #[serde(rename = "path")]
    path: Option<PathData>,
    tokens: Option<Tokens>,
    #[serde(rename = "modelID")]
    model_id: Option<String>,
    time: Option<TimeData>,
}

#[derive(Deserialize)]
struct PathData {
    cwd: Option<String>,
}

#[derive(Deserialize)]
struct Tokens {
    input: Option<i64>,
    output: Option<i64>,
    reasoning: Option<i64>,
    cache: Option<Cache>,
}

#[derive(Deserialize)]
struct Cache {
    read: Option<i64>,
    write: Option<i64>,
}

#[derive(Deserialize)]
struct TimeData {
    created: Option<i64>,
}

pub struct QoderSource {
    /// Resolved database path once, so repeated scans agree on the target.
    db: OnceLock<Option<PathBuf>>,
}

impl QoderSource {
    pub fn new(_config: ProviderConfig) -> Self {
        QoderSource {
            db: OnceLock::new(),
        }
    }

    fn db(&self) -> Option<PathBuf> {
        self.db.get_or_init(|| Some(resolve_db_path())).clone()
    }

    fn open_db(path: &std::path::Path) -> Result<Connection, String> {
        let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(c) => c,
            Err(_) => {
                let c = Connection::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
                c.pragma_update(None, "query_only", "ON")
                    .map_err(|e| format!("query_only {path:?}: {e}"))?;
                c
            }
        };
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(|e| format!("busy_timeout {path:?}: {e}"))?;
        Ok(conn)
    }

    fn scan_db(
        conn: &Connection,
        emit: &mut dyn std::ops::FnMut(UsageRecord),
    ) -> Result<(), String> {
        // Best-effort: Qoder CN may report usage with an OpenCode-compatible
        // `message.data` JSON. Any schema mismatch is swallowed so a changed
        // schema never breaks the whole scan.
        let Ok(mut stmt) =
            conn.prepare("SELECT id, session_id, time_created, data FROM message ORDER BY id")
        else {
            return Ok(());
        };
        let rows = match stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        }) {
            Ok(r) => r,
            Err(_) => return Ok(()),
        };
        for row in rows {
            let (id, session_id, created_ms, data) = match row {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Some(r) = Self::record_from_message(&id, &session_id, created_ms, &data) {
                emit(r);
            }
        }
        Ok(())
    }

    fn record_from_message(
        id: &str,
        session_id: &str,
        created_ms: i64,
        data: &str,
    ) -> Option<UsageRecord> {
        let d: MessageData = serde_json::from_str(data).ok()?;
        if d.role.as_deref() != Some("assistant") {
            return None;
        }
        let tokens = d.tokens?;
        let cache = tokens.cache.as_ref();
        let input = tokens.input.unwrap_or(0).max(0) + tokens.reasoning.unwrap_or(0).max(0);
        let output = tokens.output.unwrap_or(0).max(0);
        let cache_read = cache.and_then(|c| c.read).unwrap_or(0).max(0);
        let cache_write = cache.and_then(|c| c.write).unwrap_or(0).max(0);
        if input + output + cache_read + cache_write == 0 {
            return None;
        }
        let started_at = d
            .time
            .and_then(|t| t.created)
            .or(Some(created_ms))
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now);
        let project = d
            .path
            .and_then(|p| p.cwd)
            .and_then(|cwd| {
                ::std::path::Path::new(&cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let model = d.model_id.unwrap_or_default();
        Some(UsageRecord::new(
            Provider::Qoder,
            project,
            session_id.to_string(),
            Usage {
                model,
                started_at,
                input_tokens: input as u64,
                output_tokens: output as u64,
                cache_read_tokens: cache_read as u64,
                cache_write_tokens: cache_write as u64,
                cost_micros: 0,
            },
            data.len() as u64,
            format!("local.db:{id}"),
        ))
    }
}

#[cfg(target_os = "windows")]
fn platform_db_dir() -> std::path::PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        std::path::PathBuf::from(appdata)
            .join("QoderCN")
            .join("SharedClientCache")
            .join("cache")
            .join("db")
    } else {
        std::path::PathBuf::from(".")
    }
}

#[cfg(target_os = "macos")]
fn platform_db_dir() -> PathBuf {
    crate::platform::home_dir()
        .map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("QoderCN")
                .join("SharedClientCache")
                .join("cache")
                .join("db")
        })
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn platform_db_dir() -> std::path::PathBuf {
    crate::platform::home_dir()
        .map(|h| {
            h.join(".config")
                .join("QoderCN")
                .join("SharedClientCache")
                .join("cache")
                .join("db")
        })
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn resolve_db_path() -> std::path::PathBuf {
    if let Ok(dir) = std::env::var(QODER_DB_OVERRIDE) {
        if !dir.trim().is_empty() {
            return std::path::PathBuf::from(dir);
        }
    }
    platform_db_dir().join("local.db")
}

impl ProviderSource for QoderSource {
    fn provider(&self) -> Provider {
        Provider::Qoder
    }

    fn data_dirs(&self) -> Result<Vec<std::path::PathBuf>, ProviderError> {
        let db = self
            .db()
            .ok_or(ProviderError::DataDirNotFound(Provider::Qoder))?;
        let dir = db
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| db.clone());
        if dir.is_dir() {
            Ok(vec![dir])
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Qoder))
        }
    }

    fn scan(
        &self,
        emit: &mut dyn std::ops::FnMut(UsageRecord),
    ) -> Result<ScanOutput, ProviderError> {
        let db = self
            .db()
            .ok_or(ProviderError::DataDirNotFound(Provider::Qoder))?;
        if !db.is_file() {
            return Err(ProviderError::DataDirNotFound(Provider::Qoder));
        }
        let mut errors = Vec::new();
        let (found, max_mtime, total_bytes) = file_stats(&db);
        match Self::open_db(&db) {
            Ok(conn) => {
                if let Err(e) = Self::scan_db(&conn, emit) {
                    errors.push(e);
                }
            }
            Err(e) => errors.push(e),
        }
        Ok(ScanOutput {
            found_files: found,
            fingerprint: fingerprint(found, max_mtime, total_bytes),
            errors,
            ..Default::default()
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let db = self
            .db()
            .ok_or(ProviderError::DataDirNotFound(Provider::Qoder))?;
        if !db.is_file() {
            return Err(ProviderError::DataDirNotFound(Provider::Qoder));
        }
        let (found, max_mtime, total_bytes) = file_stats(&db);
        Ok(fingerprint(found, max_mtime, total_bytes))
    }
}

fn file_stats(path: &std::path::Path) -> (u64, i64, u64) {
    let mut found = 0u64;
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    for f in [
        path,
        &path.with_extension("db-wal"),
        &path.with_extension("db-shm"),
    ] {
        let Ok(meta) = std::fs::metadata(f) else {
            continue;
        };
        found += 1;
        total_bytes += meta.len();
        if let Ok(modified) = meta.modified() {
            if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                max_mtime = max_mtime.max(unix.as_secs() as i64);
            }
        }
    }
    (found, max_mtime, total_bytes)
}
