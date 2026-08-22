//! Codex CLI data source: parses `~/.codex/sessions/**/*.jsonl`.
//!
//! Each session file is a JSONL stream of rollout events. Token usage is
//! reported by `event_msg` events whose payload type is `token_count`; the
//! `info.last_token_usage` object carries the per-request delta since the
//! previous `token_count` event. The model in effect comes from the most
//! recent `turn_context` event, and the project comes from `session_meta`
//! `payload.cwd`. Files without any `token_count` event (e.g. short or
//! aborted sessions) simply produce no records.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
#[cfg(test)]
use super::source::scan_roots;
use super::source::{
    for_each_line, roots_fingerprint, scan_roots_incremental, FileStates, ProviderConfig,
    ProviderError, ProviderSource, ScanOutput, ScanRoot,
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
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

/// Parsing state carried across the lines of one session file.
struct SessionState {
    session_id: String,
    project: String,
    model: String,
}

impl CodexSource {
    pub fn new(config: ProviderConfig) -> Self {
        CodexSource {
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
                discover_roots(&[".codex", "sessions"])
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

    /// Stream the file line-by-line, carrying the cross-line parsing state
    /// (session id, project, current model) and emitting one record per
    /// `token_count` event without holding the raw text or the full record
    /// set in memory.
    fn parse_file(
        path: &Path,
        rel: &Path,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let mut state = SessionState {
            session_id: String::new(),
            project: String::new(),
            model: String::new(),
        };
        for_each_line(path, |line, line_idx| {
            if let Some(r) = Self::parse_line(line, &mut state, rel, line_idx) {
                emit(r);
            }
        })
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
                // TokenMonitor's buckets disjoint so `total_tokens()` stays accurate.
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

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Codex))
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
            return Err(ProviderError::DataDirNotFound(Provider::Codex));
        }
        let mut errors = Vec::new();
        let (found_files, fingerprint, file_states) = scan_roots_incremental(
            &roots,
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
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Codex));
        }
        roots_fingerprint(&roots, self.config.max_depth, self.config.max_file_size)
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
        let body = r#"{"timestamp":"2026-08-07T10:35:00.000Z","type":"session_meta","payload":{"id":"019fdbca-a8c9-7132-853b-77eb29823eeb","cwd":"C:\\Users\\yulei\\RustProjects\\TokenMonitor"}}
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

    /// Scan and collect the streamed records, mirroring how tests consume a
    /// provider.
    fn scan_collect(src: &CodexSource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_token_count_events_into_records() {
        let dir = tempdir().unwrap();
        write_session_file(dir.path());

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert_eq!(records.len(), 2, "one record per token_count event");
        assert!(out.errors.is_empty());

        // First request: input incl. cache minus cached, cache kept separate.
        let first = &records[0];
        assert_eq!(first.provider, Provider::Codex);
        assert_eq!(first.project, "TokenMonitor");
        assert_eq!(first.session_id, "019fdbca-a8c9-7132-853b-77eb29823eeb");
        assert_eq!(first.usage.model, "gpt-5.6-sol");
        assert_eq!(first.usage.input_tokens, 23823 - 11008);
        assert_eq!(first.usage.cache_read_tokens, 11008);
        assert_eq!(first.usage.cache_write_tokens, 0);
        assert_eq!(first.usage.output_tokens, 500);
        // TokenMonitor total stays consistent with Codex's reported total.
        assert_eq!(first.usage.total_tokens(), 24323);
        // Dedup key is the relative path + line index, like Claude.
        assert!(first.fingerprint.ends_with(":2"));

        let second = &records[1];
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

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert!(records.is_empty());
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::Codex)
        ));
    }

    /// Two roots (local home + a WSL distro) must both stream records into the
    /// same provider, with the labelled root's dedup fingerprints namespaced so
    /// identically-named session files in each root don't collide.
    #[test]
    fn merges_multiple_roots_with_namespaced_fingerprints() {
        let local = tempdir().unwrap();
        let wsl = tempdir().unwrap();
        // Same file name in both roots — the collision the label prevents.
        write_session_file(local.path());
        write_session_file(wsl.path());

        let config = ProviderConfig::for_provider(Provider::Codex);
        let roots = vec![
            ScanRoot {
                dir: local.path().to_path_buf(),
                label: None,
            },
            ScanRoot {
                dir: wsl.path().to_path_buf(),
                label: Some("wsl/Ubuntu-20.04/yulei".to_string()),
            },
        ];

        let mut records = Vec::new();
        let mut errors = Vec::new();
        let (found_files, _fingerprint) = scan_roots(
            &roots,
            &config,
            &mut |r| records.push(r),
            &mut errors,
            &mut |path, rel, file_emit| CodexSource::parse_file(path, rel, file_emit),
        );

        assert_eq!(found_files, 2);
        assert!(errors.is_empty());
        assert_eq!(records.len(), 4, "two token_count events per file");

        // The labelled root's fingerprints are namespaced; the primary root's
        // are not. Use a path prefix so this is separator-agnostic.
        let label = Path::new("wsl").join("Ubuntu-20.04").join("yulei");
        let primary: Vec<_> = records
            .iter()
            .filter(|r| !Path::new(&r.fingerprint).starts_with(&label))
            .collect();
        let wsl_records: Vec<_> = records
            .iter()
            .filter(|r| Path::new(&r.fingerprint).starts_with(&label))
            .collect();
        assert_eq!(primary.len(), 2);
        assert_eq!(wsl_records.len(), 2);
        assert_eq!(primary.len() + wsl_records.len(), records.len());
    }
}
