//! OpenCode data source: reads the SQLite stores at
//! `~/.local/share/opencode/opencode.db` and `opencode-stable.db` (all
//! channels), plus the pre-1.2 legacy JSON messages under
//! `storage/message/`. A message already migrated into SQLite and still
//! present as JSON is deduped by message id (`INSERT OR IGNORE`).
//!
//! OpenCode (opencode.ai) keeps every session and its messages in a local
//! SQLite database under the XDG data dir (`~/.local/share/opencode`; XDG
//! resolves to `~/.local/share` on Windows too, and inside WSL distros the
//! same path lives under each distro's home). Token usage is attached to each
//! assistant message (`message.data.tokens`), so the adapter emits one
//! `UsageRecord` per assistant message with a non-zero token count — matching
//! the per-request granularity of the Claude/Codex adapters. Billed messages
//! are append-only, so the `msg_<id>` dedup fingerprints stay stable across
//! rescans, and the store's own file stats drive the cheap change detector.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    fingerprint, ProviderConfig, ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// SQLite store file name inside each data root.
const DB_FILE: &str = "opencode.db";
/// Alternate channel of the same store (all channels ship a sibling DB).
const DB_FILE_ALT: &str = "opencode-stable.db";

/// Minimal view of a `message` row's `data` JSON. Only the fields aggregated
/// here are named; everything else — including full tool results and file
/// contents — is skipped by serde without being allocated.
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
    /// OpenCode reports reasoning tokens separately from input; TokenMonitor has no
    /// reasoning bucket, so they are folded into input (the Anthropic
    /// convention) to keep `total_tokens()` aligned with OpenCode's own totals.
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

pub struct OpenCodeSource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl OpenCodeSource {
    pub fn new(config: ProviderConfig) -> Self {
        OpenCodeSource {
            config,
            roots: OnceLock::new(),
        }
    }

    fn roots(&self) -> &[ScanRoot] {
        self.roots.get_or_init(|| {
            if let Some(dir) = &self.config.data_dir_override {
                vec![ScanRoot {
                    dir: dir.clone(),
                    label: None,
                }]
            } else {
                discover_roots(&[".local", "share", "opencode"])
            }
        })
    }

    fn existing_roots(&self) -> Vec<ScanRoot> {
        self.roots()
            .iter()
            .filter(|r| r.dir.is_dir())
            .cloned()
            .collect()
    }

    /// Open the store without writing to it. Prefers a genuinely read-only
    /// connection so TokenMonitor never contends with OpenCode's live database; when
    /// that fails (e.g. a WAL file whose `-shm` is missing, which read-only
    /// SQLite refuses), falls back to a `query_only` connection like TokenMonitor's
    /// own read path.
    fn open_db(path: &Path) -> Result<Connection, String> {
        let conn = match Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
            Ok(c) => c,
            Err(_) => {
                let c = Connection::open(path).map_err(|e| format!("open {:?}: {e}", path))?;
                c.pragma_update(None, "query_only", "ON")
                    .map_err(|e| format!("query_only {:?}: {e}", path))?;
                c
            }
        };
        conn.busy_timeout(Duration::from_secs(2))
            .map_err(|e| format!("busy_timeout {:?}: {e}", path))?;
        Ok(conn)
    }

    /// Stream one message per completed assistant message with a non-zero
    /// token count.
    fn scan_db(
        conn: &Connection,
        root: &ScanRoot,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let mut stmt = conn
            .prepare("SELECT id, session_id, time_created, data FROM message ORDER BY id")
            .map_err(|e| format!("prepare message query: {e}"))?;
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(|e| format!("query message: {e}"))?;
        for row in rows {
            let (id, session_id, created_ms, data) = row.map_err(|e| format!("row: {e}"))?;
            if let Some(r) = Self::record_from_message(&id, &session_id, created_ms, &data, root) {
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
        root: &ScanRoot,
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
                Path::new(&cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "unknown".to_string());
        let model = d.model_id.unwrap_or_default();

        // Namespace the dedup key per root so identically-named store files
        // from the local home and WSL distros never collide.
        let rel = match &root.label {
            Some(label) => Path::new(label).join(DB_FILE),
            None => Path::new(DB_FILE).to_path_buf(),
        };

        Some(UsageRecord::new(
            Provider::OpenCode,
            project,
            session_id.to_string(),
            Usage {
                model,
                started_at,
                input_tokens: input as u64,
                output_tokens: output as u64,
                cache_read_tokens: cache_read as u64,
                cache_write_tokens: cache_write as u64,
                cost_micros: 0, // pricing applied in a later pipeline stage
            },
            data.len() as u64,
            format!("{}:{id}", rel.display()),
        ))
    }

    /// Existing store files under a root (any version, incl. `opencode-stable.db`).
    fn db_files(root: &ScanRoot) -> Vec<PathBuf> {
        [DB_FILE, DB_FILE_ALT]
            .iter()
            .map(|f| root.dir.join(f))
            .filter(|p| p.is_file())
            .collect()
    }

    /// Legacy (pre-1.2) JSON message files under `root/storage/message/`.
    fn legacy_json_files(root: &ScanRoot, max_file_size: u64) -> Vec<PathBuf> {
        let dir = root.dir.join("storage").join("message");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut files = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let Ok(meta) = std::fs::metadata(&path) else {
                continue;
            };
            if meta.len() > max_file_size {
                continue;
            }
            files.push(path);
        }
        files.sort();
        files
    }

    /// Stream one assistant message from a legacy `storage/message/*.json` file.
    /// Its dedup key reuses the SQLite store's key (`<label>/opencode.db:<id>`),
    /// so a message already migrated into SQLite and still present as JSON is
    /// inserted once (`INSERT OR IGNORE`).
    fn scan_legacy_file(
        path: &Path,
        root: &ScanRoot,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let content = std::fs::read_to_string(path).map_err(|e| format!("read {path:?}: {e}"))?;
        let v: Value =
            serde_json::from_str(&content).map_err(|e| format!("parse {path:?}: {e}"))?;
        let id = v
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| path.file_stem().and_then(|s| s.to_str()).map(str::to_owned))
            .unwrap_or_default();
        let session_id = v
            .get("sessionID")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if let Some(r) = Self::record_from_message(&id, &session_id, 0, &content, root) {
            emit(r);
        }
        Ok(())
    }

    /// Change-detection stats over one root's store files (every channel DB plus
    /// its WAL sidecars) and legacy JSON message files. Shared by `scan` and
    /// `scan_fingerprint` so the cheap check and a full scan always agree.
    fn store_stats(root: &ScanRoot, max_file_size: u64) -> (u64, i64, u64) {
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for name in [DB_FILE, DB_FILE_ALT] {
            let db = root.dir.join(name);
            for f in [
                &db,
                &db.with_extension("db-wal"),
                &db.with_extension("db-shm"),
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
        }
        for p in Self::legacy_json_files(root, max_file_size) {
            let Ok(meta) = std::fs::metadata(&p) else {
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
}

impl ProviderSource for OpenCodeSource {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::OpenCode))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::OpenCode));
        }
        let mut errors = Vec::new();
        let mut found_files = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            let (found, mtime, bytes) = Self::store_stats(root, self.config.max_file_size);
            found_files += found;
            max_mtime = max_mtime.max(mtime);
            total_bytes += bytes;

            for db in Self::db_files(root) {
                let conn = match Self::open_db(&db) {
                    Ok(c) => c,
                    Err(e) => {
                        errors.push(e);
                        continue;
                    }
                };
                if let Err(e) = Self::scan_db(&conn, root, emit) {
                    errors.push(e);
                }
            }
            for p in Self::legacy_json_files(root, self.config.max_file_size) {
                if let Err(e) = Self::scan_legacy_file(&p, root, emit) {
                    errors.push(e);
                }
            }
        }
        Ok(ScanOutput {
            found_files,
            fingerprint: fingerprint(found_files, max_mtime, total_bytes),
            errors,
            ..Default::default()
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::OpenCode));
        }
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        let mut found = 0u64;
        for root in &roots {
            let (f, m, b) = Self::store_stats(root, self.config.max_file_size);
            found += f;
            max_mtime = max_mtime.max(m);
            total_bytes += b;
        }
        Ok(fingerprint(found, max_mtime, total_bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use rusqlite::{params, Connection};
    use tempfile::tempdir;

    use super::*;

    /// A completed assistant message, mirroring the real opencode `message`
    /// row's `data` JSON: tokens (with a separate reasoning bucket), model,
    /// cwd and timestamps.
    fn assistant_data(cwd: &str, input: i64, reasoning: i64, output: i64) -> String {
        format!(
            r#"{{"role":"assistant","path":{{"cwd":"{cwd}"}},"tokens":{{"input":{input},"output":{output},"reasoning":{reasoning},"cache":{{"read":0,"write":0}}}},"modelID":"gpt-5.6","time":{{"created":1786800000000,"completed":1786800000000}}}}"#
        )
    }

    /// Create a store (`opencode.db`) under `dir` with the given
    /// `(id, session_id, data)` message rows.
    fn write_store(dir: &Path, messages: &[(&str, &str, &str)]) {
        let conn = Connection::open(dir.join(DB_FILE)).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        for (id, session_id, data) in messages {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, data)
                 VALUES (?1, ?2, 1786800000000, ?3)",
                params![id, session_id, data],
            )
            .unwrap();
        }
    }

    /// Write a store under an explicit filename (e.g. the alternate channel
    /// `opencode-stable.db`).
    fn write_store_named(dir: &Path, filename: &str, messages: &[(&str, &str, &str)]) {
        let conn = Connection::open(dir.join(filename)).unwrap();
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        for (id, session_id, data) in messages {
            conn.execute(
                "INSERT INTO message (id, session_id, time_created, data)
                 VALUES (?1, ?2, 1786800000000, ?3)",
                params![id, session_id, data],
            )
            .unwrap();
        }
    }

    /// Write a legacy `storage/message/<id>.json` file with top-level `id` and
    /// `sessionID` plus the same `data` shape carried by a SQLite `message` row.
    fn write_legacy_message(dir: &Path, content: &str) {
        let msg_dir = dir.join("storage").join("message");
        fs::create_dir_all(&msg_dir).unwrap();
        let id = serde_json::from_str::<Value>(content)
            .ok()
            .and_then(|v| v.get("id").and_then(Value::as_str).map(str::to_owned))
            .unwrap_or_else(|| "anon".to_string());
        fs::write(msg_dir.join(format!("{id}.json")), content).unwrap();
    }

    fn source_for(dir: &Path) -> OpenCodeSource {
        OpenCodeSource::new(ProviderConfig {
            provider: Provider::OpenCode,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    fn scan_collect(src: &OpenCodeSource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_completed_assistant_messages_into_records() {
        let dir = tempdir().unwrap();
        write_store(
            dir.path(),
            &[
                (
                    "msg_1",
                    "ses_1",
                    &assistant_data("/home/u/proj", 100, 5, 20),
                ),
                ("msg_2", "ses_1", &assistant_data("/home/u/proj", 10, 0, 3)),
            ],
        );

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1, "one store file per root");
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 2);

        let first = &records[0];
        assert_eq!(first.provider, Provider::OpenCode);
        assert_eq!(first.project, "proj");
        assert_eq!(first.session_id, "ses_1");
        assert_eq!(first.usage.model, "gpt-5.6");
        // Reasoning is folded into input, matching OpenCode's own totals.
        assert_eq!(first.usage.input_tokens, 105);
        assert_eq!(first.usage.output_tokens, 20);
        assert_eq!(first.usage.cache_read_tokens, 0);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.total_tokens(), 125);
        // Dedup key is the store path + message id, like the line-based adapters.
        assert!(first.fingerprint.ends_with(":msg_1"));
    }

    #[test]
    fn skips_user_and_unbilled_messages() {
        let dir = tempdir().unwrap();
        write_store(
            dir.path(),
            &[
                (
                    "msg_u",
                    "ses_1",
                    r#"{"role":"user","path":{"cwd":"/p"},"time":{"created":1}}"#,
                ),
                // Zero-token assistant placeholder, still streaming or aborted.
                (
                    "msg_0",
                    "ses_1",
                    r#"{"role":"assistant","path":{"cwd":"/p"},"tokens":{"input":0,"output":0,"reasoning":0},"time":{"created":2}}"#,
                ),
                ("msg_1", "ses_1", &assistant_data("/p", 50, 0, 0)),
            ],
        );

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "ses_1");
    }

    #[test]
    fn maps_cache_buckets() {
        let dir = tempdir().unwrap();
        let data = r#"{"role":"assistant","path":{"cwd":"/p"},"tokens":{"input":10,"output":1,"reasoning":0,"cache":{"read":90,"write":5}},"modelID":"m","time":{"created":1,"completed":2}}"#;
        write_store(dir.path(), &[("msg_c", "ses_c", data)]);

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.cache_read_tokens, 90);
        assert_eq!(records[0].usage.cache_write_tokens, 5);
        assert_eq!(records[0].usage.total_tokens(), 106);
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::OpenCode)
        ));
    }

    #[test]
    fn namespaces_wsl_root_fingerprints() {
        let primary = ScanRoot {
            dir: PathBuf::from("/nonexistent/opencode"),
            label: None,
        };
        let wsl = ScanRoot {
            dir: PathBuf::from("/nonexistent"),
            label: Some("wsl/Ubuntu-20.04/zhy".to_string()),
        };
        let data = assistant_data("/p", 1, 0, 1);

        let local =
            OpenCodeSource::record_from_message("msg_1", "ses", 0, &data, &primary).unwrap();
        let remote = OpenCodeSource::record_from_message("msg_1", "ses", 0, &data, &wsl).unwrap();

        // Identical message ids in different roots must not collide. The
        // primary root's key has no separators (a plain string prefix works on
        // every platform); the WSL root's key is checked against its label as a
        // Path to stay separator-agnostic.
        assert_ne!(local.fingerprint, remote.fingerprint);
        assert!(local.fingerprint.starts_with("opencode.db:"));
        let label = Path::new("wsl").join("Ubuntu-20.04").join("zhy");
        assert!(Path::new(&remote.fingerprint).starts_with(&label));
    }

    #[test]
    fn fingerprint_changes_when_store_rewritten() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        write_store(
            dir.path(),
            &[("msg_1", "ses_1", &assistant_data("/p", 1, 0, 0))],
        );

        let fp1 = src.scan_fingerprint().unwrap();
        let fp2 = src.scan_fingerprint().unwrap();
        assert_eq!(fp1, fp2, "unchanged store keeps the same fingerprint");

        // Rewriting the store (new message) changes the fingerprint, so the
        // scheduler rescans and picks up the new record. The fingerprint only
        // has one-second resolution, so wait out the current second to ensure
        // the mtime actually differs.
        std::thread::sleep(Duration::from_secs(2));
        fs::remove_file(dir.path().join(DB_FILE)).unwrap();
        write_store(
            dir.path(),
            &[
                ("msg_1", "ses_1", &assistant_data("/p", 1, 0, 0)),
                ("msg_2", "ses_1", &assistant_data("/p", 2, 0, 0)),
            ],
        );
        assert_ne!(fp1, src.scan_fingerprint().unwrap());
    }

    #[test]
    fn reads_alternate_store_channel() {
        let dir = tempdir().unwrap();
        write_store_named(
            dir.path(),
            DB_FILE_ALT,
            &[("msg_1", "ses_1", &assistant_data("/p", 10, 5, 20))],
        );

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.total_tokens(), 35);
        // The alternate channel is counted as a found file.
        assert_eq!(out.found_files, 1);
    }

    #[test]
    fn dedups_legacy_json_against_sqlite_by_message_id() {
        let dir = tempdir().unwrap();
        // The same logical message lives in both the SQLite store and the
        // legacy JSON dir (still present after a partial migration).
        write_store(
            dir.path(),
            &[("msg_1", "ses_1", &assistant_data("/p", 1, 0, 0))],
        );
        let legacy = r#"{"id":"msg_1","sessionID":"ses_1","role":"assistant","path":{"cwd":"/p"},"tokens":{"input":1,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},"modelID":"gpt-5.6","time":{"created":1,"completed":2}}"#;
        write_legacy_message(dir.path(), legacy);

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert!(out.errors.is_empty());
        // The provider streams both sources; the storage-level
        // `INSERT OR IGNORE` collapses them because they share a fingerprint.
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].fingerprint, "opencode.db:msg_1");
        assert_eq!(records[0].fingerprint, records[1].fingerprint);
        assert_eq!(
            records[0].usage.total_tokens(),
            records[1].usage.total_tokens()
        );
    }
}
