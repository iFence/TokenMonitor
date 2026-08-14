//! Codex CLI data source: parses `~/.codex/sessions/**/*.jsonl`.
//!
//! Each session file is a JSONL stream of rollout events. Token usage is
//! reported by `event_msg` events whose payload type is `token_count`; the
//! `info.last_token_usage` object carries the per-request delta since the
//! previous `token_count` event. The model in effect comes from the most
//! recent `turn_context` event, and the project comes from `session_meta`
//! `payload.cwd`. Files without any `token_count` event (e.g. short or
//! aborted sessions) simply produce no records.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;
use walkdir::WalkDir;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::source::{
    dir_fingerprint, fingerprint, ProviderConfig, ProviderError, ProviderSource, ScanOutput,
};

/// Minimal view of a Codex rollout line. One all-optional shape covers every
/// event type; fields we don't name — e.g. large `agent_message` / tool-result
/// payloads — are skipped by serde without being allocated.
#[derive(Deserialize)]
struct CodexLine {
    #[serde(rename = "type")]
    kind: Option<Value>,
    timestamp: Option<String>,
    payload: Option<Payload>,
}

#[derive(Deserialize)]
struct Payload {
    #[serde(rename = "type")]
    kind: Option<Value>,
    session_id: Option<Value>,
    id: Option<Value>,
    cwd: Option<Value>,
    model: Option<Value>,
    info: Option<Info>,
}

#[derive(Deserialize)]
struct Info {
    last_token_usage: Option<LastTokenUsage>,
}

#[derive(Deserialize)]
struct LastTokenUsage {
    input_tokens: Option<Value>,
    cached_input_tokens: Option<Value>,
    cache_write_input_tokens: Option<Value>,
    output_tokens: Option<Value>,
}

pub struct CodexSource {
    config: ProviderConfig,
}

/// Parsing state carried across the lines of one session file.
struct SessionState {
    session_id: String,
    project: String,
    model: String,
}

impl CodexSource {
    pub fn new(config: ProviderConfig) -> Self {
        CodexSource { config }
    }

    fn base_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Codex))?;
        Ok(home.join(".codex").join("sessions"))
    }

    fn parse_file(&self, path: &Path, rel: &Path) -> Result<Vec<UsageRecord>, String> {
        let content = fs::read(path).map_err(|e| format!("read {:?}: {e}", path))?;
        let text = String::from_utf8_lossy(&content);

        let mut state = SessionState {
            session_id: String::new(),
            project: String::new(),
            model: String::new(),
        };
        let mut records = Vec::new();
        for (line_idx, line) in text.lines().enumerate() {
            if let Some(r) = Self::parse_line(line, &mut state, rel, line_idx) {
                records.push(r);
            }
        }
        Ok(records)
    }

    fn parse_line(
        line: &str,
        state: &mut SessionState,
        rel: &Path,
        line_idx: usize,
    ) -> Option<UsageRecord> {
        if line.trim().is_empty() {
            return None;
        }
        let value: CodexLine = serde_json::from_str(line).ok()?;
        match value.kind.as_ref().and_then(Value::as_str)? {
            "session_meta" => {
                let payload = value.payload.as_ref()?;
                // Newer versions expose `session_id`, older ones `id`.
                state.session_id = payload
                    .session_id
                    .as_ref()
                    .or(payload.id.as_ref())
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                state.project = payload
                    .cwd
                    .as_ref()
                    .and_then(Value::as_str)
                    .and_then(|cwd| Path::new(cwd).file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "unknown".to_string());
                None
            }
            "turn_context" => {
                // Each turn announces the model in effect for its requests.
                if let Some(model) = value
                    .payload
                    .as_ref()
                    .and_then(|p| p.model.as_ref())
                    .and_then(Value::as_str)
                {
                    state.model = model.to_string();
                }
                None
            }
            "event_msg" => {
                let payload = value.payload.as_ref()?;
                if payload.kind.as_ref().and_then(Value::as_str)? != "token_count" {
                    return None;
                }
                let usage = payload.info.as_ref()?.last_token_usage.as_ref()?;
                let input_tokens = usage
                    .input_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cached_tokens = usage
                    .cached_input_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let cache_write_tokens = usage
                    .cache_write_input_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                let output_tokens = usage
                    .output_tokens
                    .as_ref()
                    .and_then(Value::as_u64)
                    .unwrap_or(0);
                // Codex's `input_tokens` includes the cached portion; keep
                // rToken's buckets disjoint so `total_tokens()` stays accurate.
                let fresh_input = input_tokens.saturating_sub(cached_tokens);
                if fresh_input + cached_tokens + cache_write_tokens + output_tokens == 0 {
                    return None;
                }

                let started_at = value
                    .timestamp
                    .as_deref()
                    .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(Utc::now);

                Some(UsageRecord::new(
                    Provider::Codex,
                    state.project.clone(),
                    state.session_id.clone(),
                    Usage {
                        model: state.model.clone(),
                        started_at,
                        input_tokens: fresh_input,
                        output_tokens,
                        cache_read_tokens: cached_tokens,
                        cache_write_tokens,
                        cost_micros: 0, // pricing applied in a later pipeline stage
                    },
                    line.len() as u64,
                    format!("{}:{line_idx}", rel.display()),
                ))
            }
            _ => None,
        }
    }
}

impl ProviderSource for CodexSource {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        let dir = self.base_dir()?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Codex))
        }
    }

    fn scan(&self) -> Result<ScanOutput, ProviderError> {
        let dir = self.data_dir()?;
        let mut out = ScanOutput::default();
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;

        for entry in WalkDir::new(&dir)
            .max_depth(self.config.max_depth)
            .follow_links(false)
        {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    out.errors.push(format!("walk error: {e}"));
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            let is_jsonl = path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"));
            if !is_jsonl {
                continue;
            }
            let meta = match fs::metadata(path) {
                Ok(m) => m,
                Err(e) => {
                    out.errors.push(format!("metadata {:?}: {e}", path));
                    continue;
                }
            };
            if meta.len() > self.config.max_file_size {
                out.errors.push(format!("skip oversized {:?}", path));
                continue;
            }
            out.found_files += 1;
            total_bytes += meta.len();
            if let Ok(modified) = meta.modified() {
                if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                    max_mtime = max_mtime.max(unix.as_secs() as i64);
                }
            }
            let rel = path.strip_prefix(&dir).unwrap_or(path);
            match self.parse_file(path, rel) {
                Ok(records) => out.records.extend(records),
                Err(e) => out.errors.push(e),
            }
        }

        out.fingerprint = fingerprint(out.found_files, max_mtime, total_bytes);
        Ok(out)
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

    /// A session file mirroring the real Codex rollout format: session_meta,
    /// turn_context, then two `token_count` events carrying per-request deltas.
    fn write_session_file(dir: &Path) -> PathBuf {
        let path =
            dir.join("rollout-2026-08-07T18-35-19-019fdbca-a8c9-7132-853b-77eb29823eeb.jsonl");
        let body = r#"{"timestamp":"2026-08-07T10:35:00.000Z","type":"session_meta","payload":{"id":"019fdbca-a8c9-7132-853b-77eb29823eeb","cwd":"C:\\Users\\yulei\\RustProjects\\rToken"}}
{"timestamp":"2026-08-07T10:35:01.000Z","type":"turn_context","payload":{"turn_id":"t1","model":"gpt-5.6-sol"}}
{"timestamp":"2026-08-07T10:38:36.844Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":23823,"cached_input_tokens":11008,"cache_write_input_tokens":0,"output_tokens":500,"total_tokens":24323}}}}
{"timestamp":"2026-08-07T10:38:54.610Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":34478,"cached_input_tokens":23296,"cache_write_input_tokens":0,"output_tokens":701,"total_tokens":35179}}}}
"#;
        fs::write(&path, body).unwrap();
        path
    }

    fn source_for(dir: &Path) -> CodexSource {
        CodexSource::new(ProviderConfig {
            provider: Provider::Codex,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    #[test]
    fn parses_token_count_events_into_records() {
        let dir = tempdir().unwrap();
        write_session_file(dir.path());

        let out = source_for(dir.path()).scan().unwrap();
        assert_eq!(out.found_files, 1);
        assert_eq!(out.records.len(), 2, "one record per token_count event");
        assert!(out.errors.is_empty());

        // First request: input incl. cache minus cached, cache kept separate.
        let first = &out.records[0];
        assert_eq!(first.provider, Provider::Codex);
        assert_eq!(first.project, "rToken");
        assert_eq!(first.session_id, "019fdbca-a8c9-7132-853b-77eb29823eeb");
        assert_eq!(first.usage.model, "gpt-5.6-sol");
        assert_eq!(first.usage.input_tokens, 23823 - 11008);
        assert_eq!(first.usage.cache_read_tokens, 11008);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.output_tokens, 500);
        // rToken total stays consistent with Codex's reported total.
        assert_eq!(first.usage.total_tokens(), 24323);
        // Dedup key is the relative path + line index, like Claude.
        assert!(first.fingerprint.ends_with(":2"));

        let second = &out.records[1];
        assert_eq!(second.usage.input_tokens, 34478 - 23296);
        assert_eq!(second.usage.cache_read_tokens, 23296);
        assert_eq!(second.usage.output_tokens, 701);
        assert_eq!(second.usage.total_tokens(), 35179);
    }

    #[test]
    fn skips_files_without_token_count() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rollout-old.jsonl");
        fs::write(
            &path,
            r#"{"timestamp":"2026-03-25T03:08:34.801Z","type":"session_meta","payload":{"id":"old","cwd":"C:\\proj"}}
{"timestamp":"2026-03-25T03:08:34.810Z","type":"event_msg","payload":{"type":"task_started","turn_id":"t1"}}
"#,
        )
        .unwrap();

        let out = source_for(dir.path()).scan().unwrap();
        assert_eq!(out.found_files, 1);
        assert!(out.records.is_empty());
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dir().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::Codex)
        ));
    }
}
