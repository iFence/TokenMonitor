//! CodeBuddy data source: parses `~/.codebuddy/projects/**/*.jsonl`.
//!
//! Each session file is a JSONL stream of conversation and tool events. Token
//! usage is carried on the assistant-side lines only — `message` lines with
//! `role == "assistant"` and `function_call` lines. Every such line is one API
//! request (`providerData.usage.requests == 1`), so each produces one record.
//!
//! Usage is exposed in three shapes on the same line and always agrees:
//! `message.usage` (lean), `providerData.usage` (camelCase) and
//! `providerData.rawUsage` (OpenAI-style). The lean shape frequently omits the
//! cache split on `function_call` lines, so the cache read/write buckets are
//! taken from the richer `providerData` shapes when present. `input_tokens` /
//! `prompt_tokens` includes the cached **and** cache-written portions, so the
//! fresh input is computed by subtracting both — the disjoint-bucket rule shared
//! with the Codex adapter (and tokscale #1023). Model comes from
//! `providerData.requestModelId` (a `custom-local:` prefix is stripped) falling
//! back to `providerData.model`, project from the last path component of `cwd`,
//! and the session id from `sessionId`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

mod desktop;

use super::roots::discover_roots;
use super::source::{
    fingerprint, for_each_line, roots_fingerprint, scan_roots_incremental, FileStates,
    ProviderConfig, ProviderError, ProviderSource, ScanOutput, ScanRoot,
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

/// The three shapes of a CodeBuddy usage line. The lean `message.usage` is
/// always present on assistant-side lines; `providerData.usage` (camelCase) and
/// `providerData.rawUsage` (OpenAI-style) additionally carry the cache
/// read/write split and, on `function_call` lines, are the only place the cache
/// fields live.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderData {
    model: Option<String>,
    /// Effective model id; may carry a `custom-local:` prefix from a local
    /// router (e.g. `custom-local:deepseek-v4-flash`).
    request_model_id: Option<String>,
    /// Stable per-request identity, used for dedup across mirrored logs.
    message_id: Option<String>,
    trace_id: Option<String>,
    conversation_request_id: Option<String>,
    usage: Option<ProviderUsage>,
    raw_usage: Option<RawUsage>,
}

#[derive(Deserialize)]
struct UsageLine {
    input_tokens: Option<Value>,
    output_tokens: Option<Value>,
    cache_read_input_tokens: Option<Value>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProviderUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    input_tokens_details: Option<Vec<InputTokensDetail>>,
}

impl ProviderUsage {
    fn cached_tokens(&self) -> u64 {
        self.input_tokens_details
            .as_ref()
            .and_then(|details| details.first())
            .and_then(|d| d.cached_tokens)
            .unwrap_or(0)
    }
}

#[derive(Deserialize)]
struct InputTokensDetail {
    cached_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct RawUsage {
    prompt_tokens: Option<u64>,
    completion_tokens: Option<u64>,
    prompt_cache_hit_tokens: Option<u64>,
    #[serde(rename = "prompt_tokens_details")]
    prompt_tokens_details: Option<TokenDetails>,
    cache_read_input_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    prompt_cache_write_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct TokenDetails {
    cached_tokens: Option<u64>,
}

pub struct CodebuddySource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl CodebuddySource {
    pub fn new(config: ProviderConfig) -> Self {
        CodebuddySource {
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
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Codebuddy))?;
        Ok(home.join(".codebuddy").join("projects"))
    }

    #[cfg(test)]
    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        let dir = self.base_dir()?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Codebuddy))
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
                discover_roots(&[".codebuddy", "projects"])
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

        let usage = Self::resolve_usage(&value);
        if usage.input_tokens
            + usage.output_tokens
            + usage.cache_read_tokens
            + usage.cache_write_tokens
            == 0
        {
            return None;
        }

        let session_id = value.session_id.unwrap_or_default();
        let project = value
            .cwd
            .as_deref()
            .and_then(|cwd| Path::new(cwd).file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());

        // Stable per-request identity: line-number fingerprints shift when
        // CodeBuddy rewrites a session file, so key dedup on the request ids
        // the provider assigns (mirrors tokscale's messageId/traceId dedup).
        let identity = value
            .provider_data
            .as_ref()
            .and_then(|p| p.message_id.clone())
            .or_else(|| {
                value
                    .provider_data
                    .as_ref()
                    .and_then(|p| p.trace_id.clone())
            })
            .or_else(|| {
                value
                    .provider_data
                    .as_ref()
                    .and_then(|p| p.conversation_request_id.clone())
            })
            .unwrap_or_default();
        let fingerprint = if identity.is_empty() {
            format!("{}:{line_idx}", rel.display())
        } else {
            format!("{}:{identity}", rel.display())
        };

        Some(UsageRecord::new(
            Provider::Codebuddy,
            project,
            session_id,
            usage,
            line.len() as u64,
            fingerprint,
        ))
    }

    /// Resolve the disjoint token buckets from the three usage shapes, preferring
    /// the richest available source for the cache split.
    fn resolve_usage(value: &CodebuddyLine) -> Usage {
        let msg = value.message.as_ref().and_then(|m| m.usage.as_ref());
        let prov = value.provider_data.as_ref().and_then(|p| p.usage.as_ref());
        let raw = value
            .provider_data
            .as_ref()
            .and_then(|p| p.raw_usage.as_ref());

        let input = msg
            .and_then(|u| u.input_tokens.as_ref().and_then(Value::as_u64))
            .or_else(|| prov.and_then(|u| u.input_tokens))
            .or_else(|| raw.and_then(|u| u.prompt_tokens))
            .unwrap_or(0);
        let output = msg
            .and_then(|u| u.output_tokens.as_ref().and_then(Value::as_u64))
            .or_else(|| prov.and_then(|u| u.output_tokens))
            .or_else(|| raw.and_then(|u| u.completion_tokens))
            .unwrap_or(0);

        // Cache read (prompt cache hit) and cache write (cache creation). The
        // lean `message.usage` omits these on some lines, so prefer the richer
        // `providerData` shapes, which is what the reference implementation reads.
        let cache_read = raw
            .and_then(|u| u.prompt_cache_hit_tokens)
            .or_else(|| raw.and_then(|u| u.cache_read_input_tokens))
            .or_else(|| prov.map(ProviderUsage::cached_tokens))
            .or_else(|| {
                raw.and_then(|u| u.prompt_tokens_details.as_ref())
                    .and_then(|d| d.cached_tokens)
            })
            .or_else(|| {
                msg.and_then(|u| u.cache_read_input_tokens.as_ref())
                    .and_then(Value::as_u64)
            })
            .unwrap_or(0);
        let cache_write = raw
            .and_then(|u| u.cache_creation_input_tokens)
            .or_else(|| raw.and_then(|u| u.prompt_cache_write_tokens))
            .unwrap_or(0);

        let model = value
            .provider_data
            .as_ref()
            .and_then(|p| p.request_model_id.clone())
            .map(|id| id.strip_prefix("custom-local:").unwrap_or(&id).to_string())
            .or_else(|| value.provider_data.as_ref().and_then(|p| p.model.clone()))
            .unwrap_or_default();
        let started_at = value
            .timestamp
            .and_then(DateTime::<Utc>::from_timestamp_millis)
            .unwrap_or_else(Utc::now);

        // `input_tokens`/`prompt_tokens` includes the cached AND cache-written
        // portions, so subtract both to keep buckets disjoint: the sum of the
        // buckets stays equal to `total_tokens` while each is billed at its own
        // rate (cache-read discounted, cache-write at the write rate).
        let fresh_input = input.saturating_sub(cache_read).saturating_sub(cache_write);

        Usage {
            model,
            started_at,
            input_tokens: fresh_input,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_write,
            cost_micros: 0, // pricing applied in a later pipeline stage
        }
    }
}

impl ProviderSource for CodebuddySource {
    fn provider(&self) -> Provider {
        Provider::Codebuddy
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let mut dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        // Desktop (IDE) sessions live separately; only relevant when the CLI
        // data dir hasn't been explicitly overridden.
        if self.config.data_dir_override.is_none() {
            dirs.extend(desktop::existing_data_dirs());
        }
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Codebuddy))
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
        let mut errors = Vec::new();
        let mut found_files = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        let mut file_states = FileStates::new();

        let roots = self.existing_roots();
        if !roots.is_empty() {
            let (_found, fp, states) = scan_roots_incremental(
                &roots,
                &self.config,
                emit,
                &mut errors,
                &mut |path, rel, file_emit| Self::parse_file(path, rel, file_emit),
                known,
            );
            let (f, m, b) = parse_fp(&fp);
            found_files += f;
            max_mtime = max_mtime.max(m);
            total_bytes += b;
            file_states = states;
        }

        // Desktop sessions are small; a full scan each time is fine because the
        // collector dedups by fingerprint and the change detector skips when
        // nothing changed.
        if self.config.data_dir_override.is_none() {
            let (d_found, d_mtime, d_bytes, d_errors) = desktop::scan(emit);
            found_files += d_found;
            max_mtime = max_mtime.max(d_mtime);
            total_bytes += d_bytes;
            errors.extend(d_errors);
        }

        Ok(ScanOutput {
            found_files,
            fingerprint: fingerprint(found_files, max_mtime, total_bytes),
            file_states: Some(file_states),
            errors,
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let mut cli = None;
        let roots = self.existing_roots();
        if !roots.is_empty() {
            cli = Some(roots_fingerprint(
                &roots,
                self.config.max_depth,
                self.config.max_file_size,
            )?);
        }
        let desk = if self.config.data_dir_override.is_none() {
            desktop::fingerprint_if_exists()
        } else {
            None
        };
        combine_fp(cli.as_deref(), desk.as_deref())
            .ok_or(ProviderError::DataDirNotFound(Provider::Codebuddy))
    }
}

/// Split a `<found>:<max_mtime>:<total_bytes>` fingerprint into its parts so
/// the CLI and desktop fingerprints can be folded into one.
fn parse_fp(fp: &str) -> (u64, i64, u64) {
    let mut it = fp.split(':');
    let found = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let max_mtime = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let total_bytes = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (found, max_mtime, total_bytes)
}

/// Merge an optional CLI fingerprint and an optional desktop fingerprint.
fn combine_fp(cli: Option<&str>, desk: Option<&str>) -> Option<String> {
    let mut found = 0u64;
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    let mut any = false;
    for fp in [cli, desk].into_iter().flatten() {
        let (f, m, b) = parse_fp(fp);
        found += f;
        max_mtime = max_mtime.max(m);
        total_bytes += b;
        any = true;
    }
    any.then(|| fingerprint(found, max_mtime, total_bytes))
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

    #[test]
    fn reads_cache_split_from_provider_data_and_uses_stable_fingerprint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("session-rich.jsonl");
        // Real-shape lines: a `function_call` whose lean `message.usage` omits the
        // cache split (only in providerData.rawUsage), an assistant message whose
        // rawUsage carries cache-write, and a line with no providerData at all
        // (exercises the line-index fingerprint fallback).
        let body = r#"{"id":"fc1","timestamp":1786868000000,"type":"function_call","name":"EnterPlanMode","sessionId":"sess-a","cwd":"C:\\proj\\demo","providerData":{"model":"deepseek-v4-flash","requestModelId":"custom-local:deepseek-v4-flash","messageId":"msg-fn-1","traceId":"trace-fn-1","conversationRequestId":"conv-fn-1","usage":{"requests":1,"inputTokens":30000,"outputTokens":100,"totalTokens":30100,"inputTokensDetails":[{"cached_tokens":20000}],"outputTokensDetails":[]},"rawUsage":{"prompt_tokens":30000,"completion_tokens":100,"total_tokens":30100,"prompt_tokens_details":{"cached_tokens":20000},"completion_tokens_details":{"reasoning_tokens":0},"prompt_cache_hit_tokens":20000,"prompt_cache_miss_tokens":10000,"cache_read_input_tokens":20000}},"message":{"usage":{"input_tokens":30000,"output_tokens":100,"total_tokens":30100}}}
{"id":"asm1","timestamp":1786868010000,"type":"message","role":"assistant","sessionId":"sess-a","cwd":"C:\\proj\\demo","providerData":{"model":"hy4-preview","requestModelId":"hy4-preview","messageId":"msg-asm-1","traceId":"trace-asm-1","conversationRequestId":"conv-asm-1","usage":{"requests":1,"inputTokens":40000,"outputTokens":50,"totalTokens":40050,"inputTokensDetails":[{"cached_tokens":5000}],"outputTokensDetails":[{"reasoning_tokens":40}]},"rawUsage":{"prompt_tokens":40000,"completion_tokens":50,"total_tokens":40050,"prompt_tokens_details":{"cached_tokens":5000},"completion_tokens_details":{"reasoning_tokens":40},"prompt_cache_hit_tokens":5000,"prompt_cache_miss_tokens":32000,"cache_read_input_tokens":5000,"cache_creation_input_tokens":3000,"prompt_cache_write_tokens":3000,"completion_thinking_tokens":40}},"message":{"usage":{"input_tokens":40000,"output_tokens":50,"total_tokens":40050}}}
{"id":"m-no-pd","timestamp":1786868020000,"type":"message","role":"assistant","sessionId":"sess-a","cwd":"C:\\proj\\demo","message":{"usage":{"input_tokens":100,"output_tokens":5,"total_tokens":105}}}"#;
        fs::write(&path, body).unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert_eq!(records.len(), 3);
        assert!(out.errors.is_empty());

        // function_call: cache read comes from providerData.rawUsage because the
        // lean message.usage omitted it; fresh = input - cache_read.
        let first = &records[0];
        assert_eq!(
            first.usage.model, "deepseek-v4-flash",
            "custom-local: prefix stripped"
        );
        assert_eq!(first.usage.input_tokens, 30000 - 20000);
        assert_eq!(first.usage.cache_read_tokens, 20000);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.output_tokens, 100);
        assert_eq!(first.usage.total_tokens(), 30100);
        assert!(
            first.fingerprint.ends_with(":msg-fn-1"),
            "stable request-id key"
        );

        // assistant: cache-write mapped from rawUsage; fresh subtracts both.
        let second = &records[1];
        assert_eq!(second.usage.model, "hy4-preview");
        assert_eq!(second.usage.input_tokens, 40000 - 5000 - 3000);
        assert_eq!(second.usage.cache_read_tokens, 5000);
        assert_eq!(second.usage.cache_write_tokens, 3000);
        assert_eq!(second.usage.output_tokens, 50);
        assert_eq!(second.usage.total_tokens(), 40050);
        assert!(second.fingerprint.ends_with(":msg-asm-1"));

        // No providerData: no stable id, fall back to the line index.
        let third = &records[2];
        assert_eq!(third.usage.input_tokens, 100);
        assert_eq!(third.usage.cache_read_tokens, 0);
        assert!(third.fingerprint.ends_with(":2"), "line-index fallback");
    }
}
