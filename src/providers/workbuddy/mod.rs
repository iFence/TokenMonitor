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
//! the last path component of `cwd`, and the session id from `sessionId`.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::source::{
    dir_fingerprint, for_each_line, scan_jsonl_dir_incremental, FileStates, ProviderConfig,
    ProviderError, ProviderSource, ScanOutput,
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
struct ProviderData {
    model: Option<String>,
    usage: Option<UsageLine>,
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

pub struct WorkbuddySource {
    config: ProviderConfig,
}

impl WorkbuddySource {
    pub fn new(config: ProviderConfig) -> Self {
        WorkbuddySource { config }
    }

    fn base_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Workbuddy))?;
        Ok(home.join(".workbuddy").join("projects"))
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        let dir = self.base_dir()?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Workbuddy))
        }
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
        // `providerData.usage`; fall back so both parse into one record per
        // API request.
        let usage = value
            .message
            .as_ref()
            .and_then(|m| m.usage.as_ref())
            .or_else(|| value.provider_data.as_ref().and_then(|p| p.usage.as_ref()))?;
        let input_tokens = usage
            .input_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = usage
            .output_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read_tokens = usage
            .cache_read_input_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        // WorkBuddy's `input_tokens` includes the cached portion; keep rToken's
        // buckets disjoint so `total_tokens()` stays accurate.
        let fresh_input = input_tokens.saturating_sub(cache_read_tokens);
        if fresh_input + cache_read_tokens + output_tokens == 0 {
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
            .and_then(|cwd| Path::new(cwd).file_name())
            .map(|n| n.to_string_lossy().into_owned())
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
                cache_write_tokens: 0, // WorkBuddy's usage has no cache-write field
                cost_micros: 0,        // pricing applied in a later pipeline stage
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
        Ok(vec![self.data_dir()?])
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        self.scan_incremental(emit, &FileStates::new())
    }

    fn scan_incremental(
        &self,
        emit: &mut dyn FnMut(UsageRecord),
        known: &FileStates,
    ) -> Result<ScanOutput, ProviderError> {
        let dir = self.data_dir()?;
        let mut errors = Vec::new();
        let (found_files, fingerprint, file_states) = scan_jsonl_dir_incremental(
            &dir,
            &self.config,
            emit,
            &mut errors,
            &mut |path, rel, file_emit| Self::parse_file(path, rel, file_emit),
            known,
        );
        Ok(ScanOutput {
            found_files,
            fingerprint,
            file_states: Some(file_states),
            errors,
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let dir = self.data_dir()?;
        dir_fingerprint(&dir, self.config.max_depth, self.config.max_file_size)
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
}
