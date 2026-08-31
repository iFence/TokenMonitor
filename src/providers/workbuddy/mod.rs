//! WorkBuddy data source: parses `~/.workbuddy/projects/**/*.jsonl`.
//!
//! Each session file is a JSONL stream of conversation and tool events. Token
//! usage is carried on the assistant-side lines only — `message` lines with
//! `role == "assistant"` and `function_call` lines — in a `message.usage`
//! object shaped exactly like CodeBuddy's: `input_tokens`, `output_tokens`,
//! `total_tokens` and `cache_read_input_tokens`. Older sessions put the same
//! object in `providerData.usage` instead (and spell the keys either
//! snake_case or camelCase), so both locations and both spellings are
//! accepted. Every such line is one API request (`usage.requests == 1`), so
//! each produces one record.
//!
//! `input_tokens` includes the cached portion (verified against WorkBuddy's
//! `rawUsage.prompt_cache_miss_tokens`), so the fresh input is computed by
//! subtracting `cache_read_input_tokens` — the same disjoint-bucket rule as
//! the CodeBuddy adapter. Model comes from `providerData.model`, project from
//! `cwd` (see [`project_from_cwd`]), and the session id from `sessionId`.
//!
//! When `message.usage` / `providerData.usage` are absent, the OpenAI-style
//! `providerData.rawUsage` object is used instead, and cache-write tokens are
//! read from `prompt_cache_write_tokens` (or `cache_creation_input_tokens`) so
//! the bucket never silently drops cached writes.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    for_each_line, roots_fingerprint, scan_roots_incremental, FileStates, ProviderConfig,
    ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// Minimal view of a WorkBuddy session line. One all-optional shape covers
/// every event type; fields we don't name — e.g. `content` blocks, tool-result
/// payloads — are skipped by serde without being allocated.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkbuddyLine {
    #[serde(rename = "type")]
    kind: Option<Value>,
    role: Option<String>,
    /// Milliseconds since the Unix epoch (WorkBuddy timestamps are not RFC3339).
    timestamp: Option<i64>,
    session_id: Option<String>,
    cwd: Option<String>,
    message: Option<Message>,
    provider_data: Option<ProviderData>,
}

#[derive(Deserialize)]
struct Message {
    usage: Option<UsageLine>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderData {
    model: Option<String>,
    usage: Option<UsageLine>,
    raw_usage: Option<RawUsage>,
}

/// Token counts with both spellings: newer sessions use camelCase keys under
/// `providerData.usage` (`inputTokens`), older ones snake_case under
/// `providerData.usage` / `message.usage` (`input_tokens`).
#[derive(Deserialize)]
struct UsageLine {
    #[serde(alias = "inputTokens")]
    input_tokens: Option<Value>,
    #[serde(alias = "outputTokens")]
    output_tokens: Option<Value>,
    #[serde(alias = "cacheReadInputTokens")]
    cache_read_input_tokens: Option<Value>,
}

/// OpenAI-style usage carried in `providerData.rawUsage`. Note WorkBuddy writes
/// `cache_read_input_tokens` here as 0 even when the prompt was cached — the
/// real cached portion lives in `prompt_cache_hit_tokens`, so that is the field
/// treated as cache-read (and `prompt_cache_miss_tokens` stays implicit as
/// `prompt_tokens - prompt_cache_hit_tokens`).
#[derive(Deserialize)]
struct RawUsage {
    prompt_tokens: Option<Value>,
    completion_tokens: Option<Value>,
    prompt_cache_hit_tokens: Option<Value>,
    prompt_cache_write_tokens: Option<Value>,
    cache_creation_input_tokens: Option<Value>,
}

impl RawUsage {
    /// Cache-write tokens: `prompt_cache_write_tokens` when present, else
    /// `cache_creation_input_tokens`. Either spelling appears across sessions.
    fn cache_write(&self) -> Option<u64> {
        self.prompt_cache_write_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .or_else(|| {
                self.cache_creation_input_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
            })
    }
}

/// Map a WorkBuddy `cwd` to a stable project label. WorkBuddy launches each
/// session inside a fresh `<home>\WorkBuddy\<timestamp>` workspace, so the
/// trailing `YYYY-MM-DD-HH-MM-SS` segment is not a real project. When it is a
/// launch timestamp, label the project after the workspace root (its parent
/// directory); otherwise keep the real last path component (an actual repo).
fn project_from_cwd(cwd: &str) -> String {
    let path = Path::new(cwd);
    let Some(leaf) = path.file_name().and_then(|n| n.to_str()) else {
        return "unknown".to_string();
    };
    if is_workbuddy_timestamp(leaf) {
        if let Some(workspace) = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
        {
            return workspace.to_string();
        }
    }
    leaf.to_string()
}

/// Whether `s` is a WorkBuddy launch-timestamp workspace folder
/// (`YYYY-MM-DD-HH-MM-SS`), e.g. `2026-08-31-17-40-20`.
fn is_workbuddy_timestamp(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 6
        && parts[0].len() == 4
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1..]
            .iter()
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_digit()))
}

pub struct WorkbuddySource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl WorkbuddySource {
    pub fn new(config: ProviderConfig) -> Self {
        WorkbuddySource {
            config,
            roots: OnceLock::new(),
        }
    }

    #[cfg(test)]
    fn base_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Workbuddy))?;
        Ok(home.join(".workbuddy").join("projects"))
    }

    fn roots(&self) -> &[ScanRoot] {
        self.roots.get_or_init(|| {
            if let Some(dir) = &self.config.data_dir_override {
                vec![ScanRoot {
                    dir: dir.clone(),
                    label: None,
                }]
            } else {
                discover_roots(&[".workbuddy", "projects"])
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

    /// Test-only helper: the single primary (home) projects root, erroring when
    /// it does not exist.
    #[cfg(test)]
    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        let dir = self.base_dir()?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Workbuddy))
        }
    }

    /// The per-root SQLite ledger is a sibling of the `projects` dir
    /// (`~/.workbuddy/workbuddy.db`), not the scan root itself.
    fn db_path(root: &ScanRoot) -> PathBuf {
        root.dir.parent().unwrap_or(&root.dir).join("workbuddy.db")
    }

    /// Read-only open (with a `query_only` fallback for a WAL missing its
    /// `-shm`), never contending with WorkBuddy's live database.
    fn open_db(path: &Path) -> Result<Connection, String> {
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

    /// Change-detection stats over one root's SQLite ledger (plus WAL/SHM).
    fn db_stats(root: &ScanRoot) -> (u64, i64, u64) {
        let db = Self::db_path(root);
        if !db.is_file() {
            return (0, 0, 0);
        }
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
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
        (found, max_mtime, total_bytes)
    }

    /// Combine the jsonl fingerprint with the SQLite ledger signal so a change
    /// to either source triggers a rescan.
    fn combined_fingerprint(jsonl_fp: String, roots: &[ScanRoot]) -> String {
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in roots {
            let (f, m, b) = Self::db_stats(root);
            found += f;
            max_mtime = max_mtime.max(m);
            total_bytes += b;
        }
        format!("{jsonl_fp}|{found}:{max_mtime}:{total_bytes}")
    }

    /// Emit one session-level input-token record per session that has a ledger
    /// entry but no usage in the jsonl transcripts (e.g. a session that hit a
    /// quota error before writing usage lines). `covered` holds session ids
    /// already counted from jsonl, so a session is never double counted.
    fn scan_db_fallback(
        root: &ScanRoot,
        covered: &HashSet<String>,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let db = Self::db_path(root);
        if !db.is_file() {
            return Ok(());
        }
        let conn = Self::open_db(&db)?;
        let Ok(mut stmt) = conn.prepare(
            "SELECT s.id, s.cwd, s.model, s.created_at, u.used
                 FROM sessions s
                 JOIN session_usage u ON u.session_id = s.id
                 WHERE u.used > 0",
        ) else {
            // Schema changed (no sessions/session_usage tables): fall back
            // silently rather than surfacing an error every scan.
            return Ok(());
        };
        let Ok(rows) = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        }) else {
            return Ok(());
        };
        let rel = match &root.label {
            Some(label) => Path::new(label).join("workbuddy.db"),
            None => PathBuf::from("workbuddy.db"),
        };
        for row in rows {
            let (id, cwd, model, created_at, used) = row.map_err(|e| format!("row: {e}"))?;
            if covered.contains(&id) {
                continue;
            }
            let project = cwd
                .as_deref()
                .map(project_from_cwd)
                .unwrap_or_else(|| "unknown".to_string());
            let started_at = Utc
                .timestamp_millis_opt(created_at)
                .single()
                .unwrap_or_else(Utc::now);
            emit(UsageRecord::new(
                Provider::Workbuddy,
                project,
                id.clone(),
                Usage {
                    model: model.unwrap_or_default(),
                    started_at,
                    input_tokens: used.max(0) as u64,
                    output_tokens: 0,
                    cache_read_tokens: 0,
                    cache_write_tokens: 0,
                    cost_micros: 0,
                },
                0,
                format!("{}:{id}", rel.display()),
            ));
        }
        Ok(())
    }

    /// Stream the file line-by-line, emitting one record per usage-bearing
    /// line without holding the raw text or the full record set in memory.
    fn parse_file(
        path: &Path,
        rel: &Path,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        for_each_line(path, |line, line_idx| {
            if let Some(r) = Self::parse_line(line, rel, line_idx) {
                emit(r);
            }
        })
    }

    fn parse_line(line: &str, rel: &Path, line_idx: usize) -> Option<UsageRecord> {
        if line.trim().is_empty() {
            return None;
        }
        let value: WorkbuddyLine = serde_json::from_str(line).ok()?;
        // Only assistant-side lines carry usage: assistant messages and tool
        // calls. Reasoning/user/tool-result lines have no usage of their own.
        let is_assistant_message = value.kind.as_ref().and_then(Value::as_str) == Some("message")
            && value.role.as_deref() == Some("assistant");
        let is_function_call = value.kind.as_ref().and_then(Value::as_str) == Some("function_call");
        if !is_assistant_message && !is_function_call {
            return None;
        }

        // Newer sessions put usage in `message.usage`, older ones in
        // `providerData.usage`; some flows write only `providerData.rawUsage`.
        // Prefer the normalized `message.usage` / `providerData.usage`, then
        // fall back to rawUsage so every usage-bearing line becomes one record.
        let message_usage = value.message.as_ref().and_then(|m| m.usage.as_ref());
        let provider_usage = value.provider_data.as_ref().and_then(|p| p.usage.as_ref());
        let raw_usage = value
            .provider_data
            .as_ref()
            .and_then(|p| p.raw_usage.as_ref());
        let (input_tokens, output_tokens, cache_read_tokens, cache_write_tokens) =
            if let Some(usage) = message_usage.or(provider_usage) {
                let input = usage
                    .input_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let output = usage
                    .output_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cache_read = usage
                    .cache_read_input_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                // Cache-write is only present in rawUsage; default to 0 here.
                let cache_write = raw_usage.and_then(RawUsage::cache_write).unwrap_or(0);
                (input, output, cache_read, cache_write)
            } else if let Some(raw) = raw_usage {
                let input = raw
                    .prompt_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let output = raw
                    .completion_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cache_read = raw
                    .prompt_cache_hit_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cache_write = raw.cache_write().unwrap_or(0);
                (input, output, cache_read, cache_write)
            } else {
                return None;
            };
        // WorkBuddy's `input_tokens` includes the cached portion; keep TokenMonitor's
        // buckets disjoint so `total_tokens()` stays accurate.
        let fresh_input = input_tokens.saturating_sub(cache_read_tokens);
        if fresh_input + cache_read_tokens + output_tokens + cache_write_tokens == 0 {
            return None;
        }

        let started_at = value
            .timestamp
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        let session_id = value.session_id.unwrap_or_default();
        let model = value
            .provider_data
            .as_ref()
            .and_then(|p| p.model.clone())
            .unwrap_or_default();
        let project = value
            .cwd
            .as_deref()
            .map(project_from_cwd)
            .unwrap_or_else(|| "unknown".to_string());

        Some(UsageRecord::new(
            Provider::Workbuddy,
            project,
            session_id,
            Usage {
                model,
                started_at,
                input_tokens: fresh_input,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cost_micros: 0, // pricing applied in a later pipeline stage
            },
            line.len() as u64,
            format!("{}:{line_idx}", rel.display()),
        ))
    }
}

impl ProviderSource for WorkbuddySource {
    fn provider(&self) -> Provider {
        Provider::Workbuddy
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Workbuddy))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        self.scan_incremental(emit, &FileStates::new())
    }

    fn scan_incremental(
        &self,
        emit: &mut dyn FnMut(UsageRecord),
        known: &FileStates,
    ) -> Result<ScanOutput, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Workbuddy));
        }
        let mut errors = Vec::new();
        let mut covered: HashSet<String> = HashSet::new();
        let (found_files, jsonl_fp, file_states) = scan_roots_incremental(
            &roots,
            &self.config,
            &mut |r| {
                covered.insert(r.session_id.clone());
                emit(r);
            },
            &mut errors,
            &mut |path, rel, file_emit| Self::parse_file(path, rel, file_emit),
            known,
        );
        for root in &roots {
            if let Err(e) = Self::scan_db_fallback(root, &covered, emit) {
                errors.push(e);
            }
        }
        Ok(ScanOutput {
            found_files,
            fingerprint: Self::combined_fingerprint(jsonl_fp, &roots),
            file_states: Some(file_states),
            errors,
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Workbuddy));
        }
        let jsonl_fp = roots_fingerprint(&roots, self.config.max_depth, self.config.max_file_size)?;
        Ok(Self::combined_fingerprint(jsonl_fp, &roots))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use tempfile::tempdir;

    use super::*;

    /// A current-format session file mirroring real WorkBuddy output: a user
    /// message (no usage), a `function_call` with usage, and a final assistant
    /// `message` with usage. Newer sessions carry the count in both
    /// `message.usage` (snake_case) and `providerData.usage` (camelCase).
    fn write_session_file(dir: &Path) -> PathBuf {
        let path = dir.join("session-demo.jsonl");
        let body = r#"{"id":"m1","timestamp":1787053120163,"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}],"sessionId":"sess-1","cwd":"c:\\Users\\yulei\\WorkBuddy\\demo"}
{"id":"m2","timestamp":1787053133179,"type":"function_call","sessionId":"sess-1","cwd":"c:\\Users\\yulei\\WorkBuddy\\demo","providerData":{"model":"glm-5.2","usage":{"requests":1,"inputTokens":36739,"outputTokens":140,"totalTokens":36879,"inputTokensDetails":[{"cached_tokens":12160}],"outputTokensDetails":[{"reasoning_tokens":53}]}},"message":{"usage":{"input_tokens":36739,"output_tokens":140,"total_tokens":36879,"cache_read_input_tokens":12160}}}
{"id":"m3","timestamp":1787053307038,"type":"message","role":"assistant","status":"completed","sessionId":"sess-1","cwd":"c:\\Users\\yulei\\WorkBuddy\\demo","providerData":{"model":"glm-5.2","usage":{"requests":1,"inputTokens":80983,"outputTokens":485,"totalTokens":81468,"inputTokensDetails":[{"cached_tokens":80768}],"outputTokensDetails":[{"reasoning_tokens":19}]}},"message":{"usage":{"input_tokens":80983,"output_tokens":485,"total_tokens":81468,"cache_read_input_tokens":80768}}}
"#;
        fs::write(&path, body).unwrap();
        path
    }

    fn source_for(dir: &Path) -> WorkbuddySource {
        WorkbuddySource::new(ProviderConfig {
            provider: Provider::Workbuddy,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    /// Scan and collect the streamed records, mirroring how tests consume a
    /// provider.
    fn scan_collect(src: &WorkbuddySource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_assistant_and_tool_call_usage_into_records() {
        let dir = tempdir().unwrap();
        write_session_file(dir.path());

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        // user message line has no usage; function_call + assistant message do.
        assert_eq!(records.len(), 2, "one record per assistant-side usage line");
        assert!(out.errors.is_empty());

        // Tool call: input includes cache, so fresh input subtracts it.
        let first = &records[0];
        assert_eq!(first.provider, Provider::Workbuddy);
        assert_eq!(first.project, "demo");
        assert_eq!(first.session_id, "sess-1");
        assert_eq!(first.usage.model, "glm-5.2");
        assert_eq!(first.usage.input_tokens, 36739 - 12160);
        assert_eq!(first.usage.cache_read_tokens, 12160);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.output_tokens, 140);
        assert_eq!(first.usage.total_tokens(), 36879);
        assert!(first.fingerprint.ends_with(":1"));

        let second = &records[1];
        assert_eq!(second.usage.input_tokens, 80983 - 80768);
        assert_eq!(second.usage.cache_read_tokens, 80768);
        assert_eq!(second.usage.output_tokens, 485);
        assert_eq!(second.usage.total_tokens(), 81468);
    }

    /// Older sessions carry usage only in `providerData.usage` with
    /// snake_case keys, and only on assistant messages.
    #[test]
    fn parses_legacy_provider_data_usage() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session-legacy.jsonl");
        fs::write(
            &path,
            r#"{"id":"m1","type":"message","role":"user","sessionId":"sess-old","content":[{"type":"input_text","text":"hi"}]}
{"id":"m2","type":"message","role":"assistant","sessionId":"sess-old","content":[{"type":"output_text","text":"ok"}],"providerData":{"model":"auto","usage":{"input_tokens":101369,"output_tokens":633,"total_tokens":102002}}}
"#,
        )
        .unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.model, "auto");
        assert_eq!(records[0].usage.input_tokens, 101369);
        assert_eq!(records[0].usage.cache_read_tokens, 0);
        assert_eq!(records[0].usage.output_tokens, 633);
        assert_eq!(records[0].usage.total_tokens(), 102002);
    }

    #[test]
    fn skips_lines_without_usage() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session-empty.jsonl");
        fs::write(
            &path,
            r#"{"id":"m1","timestamp":1787053120163,"type":"message","role":"user","sessionId":"sess-1","cwd":"C:\\proj"}
{"id":"m2","timestamp":1787053130000,"type":"reasoning","sessionId":"sess-1","cwd":"C:\\proj"}
{"id":"m3","timestamp":1787053140000,"type":"function_call_result","name":"WebSearch","sessionId":"sess-1","cwd":"C:\\proj"}
{"id":"m4","timestamp":1787053150000,"type":"message","role":"assistant","sessionId":"sess-1","cwd":"C:\\proj","message":{"usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0,"cache_read_input_tokens":0}}}
"#,
        )
        .unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert!(
            records.is_empty(),
            "user/reasoning/tool-result/zero-usage lines yield nothing"
        );
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dir().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::Workbuddy)
        ));
    }

    /// Mirrors the real WorkBuddy session found on this machine: six
    /// `function_call` hops plus a final assistant `message`, each a separate
    /// billed request, inside a launch-timestamp workspace. Asserts the
    /// workspace-root project label and the aggregate token totals.
    #[test]
    fn parses_real_workbuddy_session_shape() {
        use serde_json::json;

        let dir = tempdir().unwrap();
        let path = dir.path().join("session-real.jsonl");
        let cwd = r"c:\Users\ZHY\WorkBuddy\2026-08-31-17-40-20";
        let rows: [(u64, u64, u64); 7] = [
            (26394, 289, 14464),
            (26699, 74, 26368),
            (37501, 305, 26688),
            (52004, 4393, 37440),
            (56440, 186, 51968),
            (56675, 82, 56384),
            (56852, 527, 56640),
        ];
        let mut lines = vec![json!({
            "type": "message",
            "role": "user",
            "sessionId": "sess-r",
            "cwd": cwd,
            "content": [{"type": "input_text", "text": "hi"}],
        })
        .to_string()];
        for (idx, (input, output, cache)) in rows.iter().enumerate() {
            let last = idx == rows.len() - 1;
            lines.push(
                json!({
                    "type": if last { "message" } else { "function_call" },
                    "role": if last { "assistant" } else { "user" },
                    "sessionId": "sess-r",
                    "cwd": cwd,
                    "providerData": {
                        "model": "glm-5.3",
                        "rawUsage": {
                            "prompt_tokens": input,
                            "completion_tokens": output,
                            "prompt_cache_hit_tokens": cache,
                            "prompt_cache_write_tokens": 0,
                        },
                    },
                    "message": {
                        "usage": {
                            "input_tokens": input,
                            "output_tokens": output,
                            "cache_read_input_tokens": cache,
                        },
                    },
                })
                .to_string(),
            );
        }
        fs::write(&path, lines.join("\n")).unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert_eq!(records.len(), 7, "one record per billed request");
        assert!(out.errors.is_empty());

        let (mut input, mut cache, mut output, mut write, mut total) = (0, 0, 0, 0, 0);
        for r in &records {
            assert_eq!(r.project, "WorkBuddy");
            assert_eq!(r.usage.model, "glm-5.3");
            input += r.usage.input_tokens;
            cache += r.usage.cache_read_tokens;
            output += r.usage.output_tokens;
            write += r.usage.cache_write_tokens;
            total += r.usage.total_tokens();
        }
        assert_eq!(input, 312_565 - 269_952, "fresh input");
        assert_eq!(cache, 269_952, "cache-read input");
        assert_eq!(output, 5_856, "output");
        assert_eq!(write, 0, "cache-write");
        assert_eq!(total, 318_421, "total tokens");
    }

    /// A request whose only usage is `providerData.rawUsage` (no
    /// `message.usage` / `providerData.usage`) still produces a record, and
    /// cache-read is taken from `prompt_cache_hit_tokens` (the field WorkBuddy
    /// actually fills) while cache-write comes from `prompt_cache_write_tokens`.
    #[test]
    fn falls_back_to_raw_usage_and_reads_cache_buckets() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session-raw.jsonl");
        fs::write(
            &path,
            r#"{"id":"m1","timestamp":1788169446546,"type":"function_call","name":"Write","sessionId":"s-raw","cwd":"c:\\Users\\ZHY\\WorkBuddy\\2026-08-31-17-40-20","providerData":{"model":"glm-5.3","rawUsage":{"prompt_tokens":56440,"completion_tokens":186,"prompt_cache_hit_tokens":51968,"prompt_cache_write_tokens":1200}}}
"#,
        )
        .unwrap();

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.usage.input_tokens, 56_440 - 51_968);
        assert_eq!(r.usage.cache_read_tokens, 51_968);
        assert_eq!(r.usage.cache_write_tokens, 1_200);
        assert_eq!(r.usage.output_tokens, 186);
        assert_eq!(r.usage.total_tokens(), 4_472 + 51_968 + 1_200 + 186);
    }

    /// When both `message.usage` and `rawUsage` are present (the common case),
    /// input/output/cache-read come from `message.usage` but cache-write is
    /// still read from `rawUsage.prompt_cache_write_tokens`.
    #[test]
    fn reads_cache_write_from_raw_usage_even_when_message_usage_present() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session-both.jsonl");
        fs::write(
            &path,
            r#"{"id":"m1","timestamp":1788169446546,"type":"function_call","sessionId":"s-b","cwd":"c:\\proj","providerData":{"model":"glm-5.3","rawUsage":{"prompt_tokens":56440,"completion_tokens":186,"prompt_cache_hit_tokens":51968,"prompt_cache_write_tokens":900}},"message":{"usage":{"input_tokens":56440,"output_tokens":186,"cache_read_input_tokens":51968}}}
"#,
        )
        .unwrap();

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        let r = &records[0];
        assert_eq!(r.usage.input_tokens, 56_440 - 51_968);
        assert_eq!(r.usage.cache_read_tokens, 51_968);
        assert_eq!(r.usage.cache_write_tokens, 900);
        assert_eq!(r.usage.output_tokens, 186);
        assert_eq!(r.usage.total_tokens(), 4_472 + 51_968 + 900 + 186);
    }

    #[test]
    fn project_labels_workspace_root_for_timestamp_and_last_segment_for_repo() {
        // WorkBuddy launch workspace: label the stable workspace root instead of
        // the volatile timestamp folder.
        assert_eq!(
            project_from_cwd(r"c:\Users\ZHY\WorkBuddy\2026-08-31-17-40-20"),
            "WorkBuddy"
        );
        // A real repo path keeps its last component.
        assert_eq!(project_from_cwd(r"c:\Users\ZHY\IdeaProjects\demo"), "demo");
        assert_eq!(project_from_cwd(r"D:\src\TokenMonitor"), "TokenMonitor");
        assert!(is_workbuddy_timestamp("2026-08-31-17-40-20"));
        assert!(!is_workbuddy_timestamp("demo"));
        assert!(!is_workbuddy_timestamp("2026-8-31-17-40-20"));
        assert!(!is_workbuddy_timestamp("2026-08-31"));
    }

    #[test]
    fn sqlite_fallback_counts_sessions_without_jsonl_usage() {
        let root = tempdir().unwrap();
        let projects = root.path().join("projects");
        fs::create_dir_all(&projects).unwrap();
        // A session whose jsonl has no usage line (e.g. it hit a quota error
        // before writing usage) so it is not covered by the transcript scan.
        fs::write(
            projects.join("s.sql.jsonl"),
            r#"{"id":"m1","timestamp":1788169225430,"type":"message","role":"user","sessionId":"sess-x","cwd":"c:\\Users\\ZHY\\WorkBuddy\\2026-08-31-17-40-20","content":[{"type":"input_text","text":"hi"}]}
"#,
        )
        .unwrap();

        // The SQLite ledger records the real per-session input tokens.
        let conn = Connection::open(root.path().join("workbuddy.db")).unwrap();
        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                cwd TEXT NOT NULL,
                user_id TEXT,
                model TEXT,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE session_usage (
                session_id TEXT PRIMARY KEY,
                used INTEGER NOT NULL,
                size INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO sessions (id, cwd, user_id, model, created_at)
             VALUES (?1, ?2, 'u', 'auto', 1788169225557)",
            rusqlite::params!["sess-x", r"c:\Users\ZHY\WorkBuddy\2026-08-31-17-40-20"],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session_usage (session_id, used, size, updated_at)
             VALUES (?1, 56852, 168000, 1788169465879)",
            rusqlite::params!["sess-x"],
        )
        .unwrap();

        let (_, records) = scan_collect(&source_for(&projects));
        assert_eq!(records.len(), 1, "the ledger-only session is counted once");
        assert_eq!(records[0].session_id, "sess-x");
        assert_eq!(records[0].usage.input_tokens, 56852);
        // WorkBuddy launch workspace: labelled by its workspace root.
        assert_eq!(records[0].project, "WorkBuddy");
        assert!(records[0].fingerprint.starts_with("workbuddy.db:sess-x"));
    }
}
