//! CodeBuddy data source: parses `~/.codebuddy/projects/**/*.jsonl`.
//!
//! Each session file is a JSONL stream of conversation and tool events. Token
//! usage is carried on the assistant-side lines only — `message` lines with
//! `role == "assistant"` and `function_call` lines — in a `message.usage`
//! object shaped exactly like Claude Code's: `input_tokens`, `output_tokens`,
//! `total_tokens` and `cache_read_input_tokens`. Every such line is one API
//! request (`providerData.usage.requests == 1`), so each produces one record.
//!
//! `input_tokens` includes the cached portion (verified against CodeBuddy's
//! `rawUsage.prompt_cache_miss_tokens`), so the fresh input is computed by
//! subtracting `cache_read_input_tokens` — the same disjoint-bucket rule as the
//! Codex adapter. Model comes from `providerData.model`, project from the last
//! path component of `cwd`, and the session id from `sessionId`.

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

/// Minimal view of a CodeBuddy session line. One all-optional shape covers every
/// event type; fields we don't name — e.g. `content` blocks, tool-result
/// payloads — are skipped by serde without being allocated.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodebuddyLine {
    #[serde(rename = "type")]
    kind: Option<Value>,
    role: Option<String>,
    /// Milliseconds since the Unix epoch (CodeBuddy timestamps are not RFC3339).
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
}

#[derive(Deserialize)]
struct UsageLine {
    input_tokens: Option<Value>,
    output_tokens: Option<Value>,
    cache_read_input_tokens: Option<Value>,
}

pub struct CodebuddySource {
    config: ProviderConfig,
}

impl CodebuddySource {
    pub fn new(config: ProviderConfig) -> Self {
        CodebuddySource { config }
    }

    fn base_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Codebuddy))?;
        Ok(home.join(".codebuddy").join("projects"))
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        let dir = self.base_dir()?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Codebuddy))
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
        let value: CodebuddyLine = serde_json::from_str(line).ok()?;
        // Only assistant-side lines carry usage: assistant messages and tool
        // calls. Reasoning/user/tool-result lines have no usage of their own.
        let is_assistant_message = value.kind.as_ref().and_then(Value::as_str) == Some("message")
            && value.role.as_deref() == Some("assistant");
        let is_function_call = value.kind.as_ref().and_then(Value::as_str) == Some("function_call");
        if !is_assistant_message && !is_function_call {
            return None;
        }

        let usage = value.message.as_ref()?.usage.as_ref()?;
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
        // CodeBuddy's `input_tokens` includes the cached portion; keep rToken's
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
            Provider::Codebuddy,
            project,
            session_id,
            Usage {
                model,
                started_at,
                input_tokens: fresh_input,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens: 0, // CodeBuddy's message.usage has no cache-write field
                cost_micros: 0,        // pricing applied in a later pipeline stage
            },
            line.len() as u64,
            format!("{}:{line_idx}", rel.display()),
        ))
    }
}

impl ProviderSource for CodebuddySource {
    fn provider(&self) -> Provider {
        Provider::Codebuddy
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

    /// A session file mirroring the real CodeBuddy format: a user message (no
    /// usage), a `function_call` with usage, and a final assistant `message`
    /// with usage.
    fn write_session_file(dir: &Path) -> PathBuf {
        let path = dir.join("session-demo.jsonl");
        let body = r#"{"id":"m1","timestamp":1784511821824,"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}],"sessionId":"sess-1","cwd":"C:\\Users\\yulei\\IdeaProjects\\demo"}
{"id":"m2","timestamp":1784511830000,"type":"function_call","name":"Read","sessionId":"sess-1","cwd":"C:\\Users\\yulei\\IdeaProjects\\demo","providerData":{"model":"glm-5.2"},"message":{"usage":{"input_tokens":33112,"output_tokens":64,"total_tokens":33176,"cache_read_input_tokens":3968}}}
{"id":"m3","timestamp":1784511840000,"type":"message","role":"assistant","sessionId":"sess-1","cwd":"C:\\Users\\yulei\\IdeaProjects\\demo","providerData":{"model":"glm-5.2"},"message":{"usage":{"input_tokens":72149,"output_tokens":182,"total_tokens":72331,"cache_read_input_tokens":70592}}}
"#;
        fs::write(&path, body).unwrap();
        path
    }

    fn source_for(dir: &Path) -> CodebuddySource {
        CodebuddySource::new(ProviderConfig {
            provider: Provider::Codebuddy,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    /// Scan and collect the streamed records, mirroring how tests consume a
    /// provider.
    fn scan_collect(src: &CodebuddySource) -> (ScanOutput, Vec<UsageRecord>) {
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
        assert_eq!(first.provider, Provider::Codebuddy);
        assert_eq!(first.project, "demo");
        assert_eq!(first.session_id, "sess-1");
        assert_eq!(first.usage.model, "glm-5.2");
        assert_eq!(first.usage.input_tokens, 33112 - 3968);
        assert_eq!(first.usage.cache_read_tokens, 3968);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.output_tokens, 64);
        assert_eq!(first.usage.total_tokens(), 33176);
        assert!(first.fingerprint.ends_with(":1"));

        let second = &records[1];
        assert_eq!(second.usage.input_tokens, 72149 - 70592);
        assert_eq!(second.usage.cache_read_tokens, 70592);
        assert_eq!(second.usage.output_tokens, 182);
        assert_eq!(second.usage.total_tokens(), 72331);
    }

    #[test]
    fn skips_lines_without_usage() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session-empty.jsonl");
        fs::write(
            &path,
            r#"{"id":"m1","timestamp":1784511821824,"type":"message","role":"user","content":[{"type":"input_text","text":"hi"}],"sessionId":"sess-1","cwd":"C:\\proj"}
{"id":"m2","timestamp":1784511830000,"type":"reasoning","sessionId":"sess-1","cwd":"C:\\proj"}
{"id":"m3","timestamp":1784511840000,"type":"message","role":"assistant","sessionId":"sess-1","cwd":"C:\\proj","message":{"usage":{"input_tokens":0,"output_tokens":0,"total_tokens":0,"cache_read_input_tokens":0}}}
"#,
        )
        .unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert!(
            records.is_empty(),
            "user/reasoning/zero-usage lines yield nothing"
        );
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dir().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::Codebuddy)
        ));
    }
}
