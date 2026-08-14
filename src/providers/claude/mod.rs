//! Claude Code data source: parses `~/.claude/projects/**/*.jsonl`.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

pub struct ClaudeSource {
    config: ProviderConfig,
}

impl ClaudeSource {
    pub fn new(config: ProviderConfig) -> Self {
        ClaudeSource { config }
    }

    fn base_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Claude))?;
        Ok(home.join(".claude").join("projects"))
    }

    fn parse_file(&self, path: &Path, rel: &Path) -> Result<Vec<UsageRecord>, String> {
        let content = fs::read(path).map_err(|e| format!("read {:?}: {e}", path))?;
        let text = String::from_utf8_lossy(&content);
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

        let mut records = Vec::new();
        for (line_idx, line) in text.lines().enumerate() {
            if let Some(r) = Self::parse_line(line, &project, &session_id, rel, line_idx) {
                records.push(r);
            }
        }
        Ok(records)
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
        let value: Value = serde_json::from_str(line).ok()?;
        if value.get("type")?.as_str()? != "assistant" {
            return None;
        }
        let usage = value.pointer("/message/usage")?;
        if usage.is_null() {
            return None;
        }
        let input_tokens = usage
            .get("input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let output_tokens = usage
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_write_tokens = usage
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cache_read_tokens = usage
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if input_tokens + output_tokens + cache_write_tokens + cache_read_tokens == 0 {
            return None;
        }

        let started_at = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        let model = value
            .pointer("/message/model")
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

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        let dir = self.base_dir()?;
        if dir.is_dir() {
            Ok(dir)
        } else {
            Err(ProviderError::DataDirNotFound(Provider::Claude))
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

        out.fingerprint = format!("{}:{max_mtime}:{total_bytes}", out.found_files);
        Ok(out)
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(self.scan()?.fingerprint)
    }
}
