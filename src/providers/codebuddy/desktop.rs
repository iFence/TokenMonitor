//! CodeBuddy desktop/IDE usage: parses the CodeBuddy IDE extension's per-request
//! token usage.
//!
//! The IDE stores conversations as JSON under
//! `%LOCALAPPDATA%\CodeBuddyExtension\Data\<profile>\CodeBuddyIDE\<profile>\history\<workspace>\<conversation>\`,
//! with usage in each conversation `index.json` (`requests[].usage`), the real
//! workspace path in `%APPDATA%\CodeBuddy CN\codebuddy-sessions.vscdb`
//! (`session:<conversation>` -> `cwd`), and the per-request model in each message
//! file's `extra` field.
//!
//! Buckets mirror the CLI adapter: `inputTokens` includes cache read + cache
//! write, so fresh input = `inputTokens - cacheTokens - cachedWriteTokens`
//! (`cacheTokens` = cache read, `cachedWriteTokens` = cache write).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

#[derive(Deserialize)]
struct ConversationIndex {
    #[serde(default)]
    requests: Vec<Request>,
}

#[derive(Deserialize)]
struct Request {
    id: String,
    #[serde(rename = "startedAt")]
    started_at: Option<i64>,
    usage: Option<RequestUsage>,
}

#[derive(Deserialize)]
struct RequestUsage {
    #[serde(rename = "inputTokens")]
    input_tokens: Option<u64>,
    #[serde(rename = "outputTokens")]
    output_tokens: Option<u64>,
    #[serde(rename = "cacheTokens")]
    cache_tokens: Option<u64>,
    #[serde(rename = "cachedWriteTokens")]
    cache_write_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct MessageFile {
    #[serde(default)]
    extra: Option<String>,
}

#[derive(Deserialize)]
struct Extra {
    #[serde(rename = "requestId")]
    request_id: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
}

/// Directory holding the CodeBuddy IDE extension data across platforms. Returns
/// every candidate that exists; a wrong candidate simply contributes nothing.
pub(crate) fn existing_data_dirs() -> Vec<PathBuf> {
    candidates().into_iter().filter(|d| d.is_dir()).collect()
}

fn candidates() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    #[cfg(target_os = "windows")]
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("CodeBuddyExtension").join("Data"));
    }
    #[cfg(target_os = "macos")]
    if let Ok(home) = crate::platform::home_dir() {
        dirs.push(
            home.join("Library")
                .join("Application Support")
                .join("CodeBuddyExtension")
                .join("Data"),
        );
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    if let Ok(home) = crate::platform::home_dir() {
        dirs.push(
            home.join(".local")
                .join("share")
                .join("CodeBuddyExtension")
                .join("Data"),
        );
    }
    dirs
}

/// `session:<conversation>` -> `cwd`, loaded once from the IDE's session DB.
fn load_cwd_map() -> HashMap<String, String> {
    let Some(db) = vscdb_path() else {
        return HashMap::new();
    };
    let conn = match Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };
    conn.busy_timeout(Duration::from_secs(2)).ok();
    let Ok(mut stmt) = conn.prepare("SELECT key, value FROM ItemTable") else {
        return HashMap::new();
    };
    let rows = stmt.query_map([], |r| {
        let key: String = r.get(0)?;
        let value_bytes: &[u8] = match r.get_ref(1)? {
            rusqlite::types::ValueRef::Text(t) => t,
            rusqlite::types::ValueRef::Blob(b) => b,
            _ => return Ok((key, String::new())),
        };
        Ok((key, String::from_utf8_lossy(value_bytes).into_owned()))
    });
    let Ok(rows) = rows else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for row in rows.flatten() {
        let (key, value) = row;
        let Some(conv) = key.strip_prefix("session:") else {
            continue;
        };
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&value) {
            if let Some(cwd) = v.get("cwd").and_then(|c| c.as_str()) {
                map.insert(conv.to_string(), cwd.to_string());
            }
        }
    }
    map
}

#[cfg(target_os = "windows")]
fn vscdb_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| {
        PathBuf::from(a)
            .join("CodeBuddy CN")
            .join("codebuddy-sessions.vscdb")
    })
}

#[cfg(not(target_os = "windows"))]
fn vscdb_path() -> Option<PathBuf> {
    None
}

fn find_index_json(dir: &Path, acc: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            find_index_json(&p, acc);
        } else if entry.file_name().to_str() == Some("index.json") {
            acc.push(p);
        }
    }
}

/// Cheap change detector over every desktop `index.json`.
pub(crate) fn fingerprint_if_exists() -> Option<String> {
    let dirs = existing_data_dirs();
    if dirs.is_empty() {
        return None;
    }
    let (found, max_mtime, total_bytes) = stats(&dirs);
    Some(super::super::source::fingerprint(
        found,
        max_mtime,
        total_bytes,
    ))
}

fn stats(dirs: &[PathBuf]) -> (u64, i64, u64) {
    let mut files = Vec::new();
    for dir in dirs {
        find_index_json(dir, &mut files);
    }
    let mut found = 0u64;
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    for f in files {
        let Ok(meta) = std::fs::metadata(&f) else {
            continue;
        };
        found += 1;
        total_bytes += meta.len();
        if let Ok(m) = meta.modified() {
            if let Ok(u) = m.duration_since(std::time::UNIX_EPOCH) {
                max_mtime = max_mtime.max(u.as_secs() as i64);
            }
        }
    }
    (found, max_mtime, total_bytes)
}

/// Scan every desktop conversation index and stream the records. Returns
/// `(found_files, max_mtime, total_bytes, errors)` so the caller folds them
/// into one provider fingerprint.
pub(crate) fn scan(emit: &mut dyn FnMut(UsageRecord)) -> (u64, i64, u64, Vec<String>) {
    let dirs = existing_data_dirs();
    if dirs.is_empty() {
        return (0, 0, 0, Vec::new());
    }
    let cwd_map = load_cwd_map();
    let mut errors = Vec::new();
    let mut files = Vec::new();
    for dir in &dirs {
        find_index_json(dir, &mut files);
    }

    let mut found = 0u64;
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;

    for idx in files {
        let Ok(meta) = std::fs::metadata(&idx) else {
            continue;
        };
        found += 1;
        total_bytes += meta.len();
        if let Ok(m) = meta.modified() {
            if let Ok(u) = m.duration_since(std::time::UNIX_EPOCH) {
                max_mtime = max_mtime.max(u.as_secs() as i64);
            }
        }

        let Some(conv_dir) = idx.parent() else {
            continue;
        };
        let Some(workspace_dir) = conv_dir.parent() else {
            continue;
        };
        let Some(conv_id) = conv_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let workspace = workspace_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("workspace");

        let Ok(text) = std::fs::read_to_string(&idx) else {
            errors.push(format!("desktop read {}", idx.display()));
            continue;
        };
        let Ok(index) = serde_json::from_str::<ConversationIndex>(&text) else {
            errors.push(format!("desktop parse {}", idx.display()));
            continue;
        };
        if index.requests.is_empty() {
            continue;
        }

        let models = load_request_models(conv_dir);
        let project = cwd_map
            .get(conv_id)
            .and_then(|cwd| {
                Path::new(cwd)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| workspace.to_string());

        for req in index.requests {
            if let Some(r) = record(conv_id, &project, workspace, &req, &models, meta.len()) {
                emit(r);
            }
        }
    }

    (found, max_mtime, total_bytes, errors)
}

fn load_request_models(conv_dir: &Path) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let msg_dir = conv_dir.join("messages");
    let Ok(entries) = std::fs::read_dir(&msg_dir) else {
        return map;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if !p
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e == "json")
        {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else {
            continue;
        };
        let Ok(msg) = serde_json::from_str::<MessageFile>(&text) else {
            continue;
        };
        let Some(extra) = msg.extra else {
            continue;
        };
        let Ok(extra) = serde_json::from_str::<Extra>(&extra) else {
            continue;
        };
        if let (Some(rid), Some(mid)) = (extra.request_id, extra.model_id) {
            map.entry(rid).or_insert(mid);
        }
    }
    map
}

fn record(
    conv_id: &str,
    project: &str,
    workspace: &str,
    req: &Request,
    models: &HashMap<String, String>,
    raw_bytes: u64,
) -> Option<UsageRecord> {
    let usage = req.usage.as_ref()?;
    let input = usage.input_tokens.unwrap_or(0);
    let output = usage.output_tokens.unwrap_or(0);
    let cache_read = usage.cache_tokens.unwrap_or(0);
    let cache_write = usage.cache_write_tokens.unwrap_or(0);
    let fresh = input.saturating_sub(cache_read).saturating_sub(cache_write);
    if fresh + output + cache_read + cache_write == 0 {
        return None;
    }
    let model = models.get(&req.id).cloned().unwrap_or_default();
    let started_at = req
        .started_at
        .and_then(|ms| DateTime::<Utc>::from_timestamp_millis(ms))
        .unwrap_or_else(Utc::now);
    Some(UsageRecord::new(
        Provider::Codebuddy,
        project.to_string(),
        conv_id.to_string(),
        Usage {
            model,
            started_at,
            input_tokens: fresh,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            cost_micros: 0,
        },
        raw_bytes,
        format!("desktop:{workspace}:{conv_id}:{}", req.id),
    ))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_request_usage_into_disjoint_buckets() {
        let text = r#"{"messages":[],"requests":[{"id":"r1","type":"craft","state":"complete","startedAt":1788231939705,"messages":["a","b"],"usage":{"inputTokens":155292,"outputTokens":610,"totalTokens":155902,"lastTokens":22655,"cacheTokens":140096,"cachedWriteTokens":0,"cachedMissTokens":15196,"credit":0}}]}"#;
        let index: ConversationIndex = serde_json::from_str(text).unwrap();
        assert_eq!(index.requests.len(), 1);

        let mut models = HashMap::new();
        models.insert("r1".to_string(), "hy4-preview".to_string());
        let r = record("conv1", "proj1", "ws1", &index.requests[0], &models, 0).unwrap();

        assert_eq!(r.project, "proj1");
        assert_eq!(r.session_id, "conv1");
        assert_eq!(r.usage.model, "hy4-preview");
        // inputTokens includes cache read + cache write.
        assert_eq!(r.usage.input_tokens, 155292 - 140096 - 0);
        assert_eq!(r.usage.cache_read_tokens, 140096);
        assert_eq!(r.usage.cache_write_tokens, 0);
        assert_eq!(r.usage.output_tokens, 610);
        assert_eq!(r.usage.total_tokens(), 155902);
        assert_eq!(r.fingerprint, "desktop:ws1:conv1:r1");
    }

    #[test]
    fn reads_model_from_message_extra() {
        let dir = tempdir().unwrap();
        let msg_dir = dir.path().join("messages");
        std::fs::create_dir_all(&msg_dir).unwrap();
        std::fs::write(
            msg_dir.join("m1.json"),
            r#"{"role":"assistant","message":"{}","id":"m1","extra":"{\"requestId\":\"r1\",\"modelId\":\"hy3\",\"modelName\":\"Hy3\"}"}"#,
        )
        .unwrap();
        let map = load_request_models(dir.path());
        assert_eq!(map.get("r1").map(String::as_str), Some("hy3"));
    }

    #[test]
    fn skips_zero_usage_request() {
        let text = r#"{"requests":[{"id":"r0","startedAt":1,"usage":{"inputTokens":0,"outputTokens":0,"cacheTokens":0,"cachedWriteTokens":0}}]}"#;
        let index: ConversationIndex = serde_json::from_str(text).unwrap();
        let models = HashMap::new();
        assert!(record("c", "p", "w", &index.requests[0], &models, 0).is_none());
    }
}
