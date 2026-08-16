//! Claude Code data source: parses `~/.claude/projects/**/*.jsonl`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    for_each_line, roots_fingerprint, scan_roots_incremental, FileStates, ProviderConfig,
    ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// Minimal view of a Claude Code JSONL line. Only the fields we aggregate are
/// named; everything else — notably the huge `message.content` array holding
/// full tool results and file contents — is skipped by serde without being
/// allocated.
#[derive(Deserialize)]
struct ClaudeLine {
    #[serde(rename = "type")]
    kind: Option<Value>,
    timestamp: Option<String>,
    message: Option<Message>,
}

#[derive(Deserialize)]
struct Message {
    model: Option<Value>,
    usage: Option<UsageLine>,
}

/// Only the token counters we aggregate; the rest of `usage.*` is skipped.
#[derive(Deserialize)]
struct UsageLine {
    input_tokens: Option<Value>,
    output_tokens: Option<Value>,
    cache_creation_input_tokens: Option<Value>,
    cache_read_input_tokens: Option<Value>,
}

pub struct ClaudeSource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl ClaudeSource {
    pub fn new(config: ProviderConfig) -> Self {
        ClaudeSource {
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
                discover_roots(&[".claude", "projects"])
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
        // TODO: decode the Claude Code project slug (path with '/' and ':' -> '-')
        let project = rel
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());
        let session_id = rel
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();

        for_each_line(path, |line, line_idx| {
            if let Some(r) = Self::parse_line(line, &project, &session_id, rel, line_idx) {
                emit(r);
            }
        })
    }

    fn parse_line(
        line: &str,
        project: &str,
        session_id: &str,
        rel: &Path,
        line_idx: usize,
    ) -> Option<UsageRecord> {
        if line.trim().is_empty() {
            return None;
        }
        let value: ClaudeLine = serde_json::from_str(line).ok()?;
        if value.kind.as_ref().and_then(Value::as_str)? != "assistant" {
            return None;
        }
        let message = value.message.as_ref()?;
        let usage = message.usage.as_ref()?;
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
        let cache_write_tokens = usage
            .cache_creation_input_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read_tokens = usage
            .cache_read_input_tokens
            .as_ref()
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input_tokens + output_tokens + cache_write_tokens + cache_read_tokens == 0 {
            return None;
        }

        let started_at = value
            .timestamp
            .as_deref()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let model = message
            .model
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        Some(UsageRecord::new(
            Provider::Claude,
            project.to_string(),
            session_id.to_string(),
            Usage {
                model,
                started_at,
                input_tokens,
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

impl ProviderSource for ClaudeSource {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Claude))
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
            return Err(ProviderError::DataDirNotFound(Provider::Claude));
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
            return Err(ProviderError::DataDirNotFound(Provider::Claude));
        }
        roots_fingerprint(&roots, self.config.max_depth, self.config.max_file_size)
    }
}
