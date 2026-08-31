//! Gemini CLI data source: parses chat transcripts under
//! `$GEMINI_CLI_HOME/tmp/*/chats/*.json` (fallback `~/.gemini/tmp/*/chats/*.json`).
//!
//! Each chat file is a JSON conversation; per-call usage lives in
//! `usageMetadata` objects (`promptTokenCount`, `candidatesTokenCount`,
//! `cachedContentTokenCount`). One `UsageRecord` is emitted per `usageMetadata`
//! found, mapping prompt -> input (the cached portion is split out), candidates
//! -> output, cached -> cache-read. Files without usage metadata, or malformed
//! ones, are skipped silently so the adapter never fails a scan.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{TimeZone, Utc};
use serde_json::Value;
use walkdir::WalkDir;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::roots::discover_roots;
use super::source::{
    fingerprint, ProviderConfig, ProviderError, ProviderSource, ScanOutput, ScanRoot,
};

/// Gemini home environment override.
const GEMINI_CLI_HOME: &str = "GEMINI_CLI_HOME";

pub struct GeminiSource {
    config: ProviderConfig,
    /// Discovered scan roots, computed lazily on the first scan (background
    /// thread) so WSL discovery never blocks UI startup.
    roots: OnceLock<Vec<ScanRoot>>,
}

impl GeminiSource {
    pub fn new(config: ProviderConfig) -> Self {
        GeminiSource {
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
            } else if let Ok(home) = std::env::var(GEMINI_CLI_HOME) {
                if home.trim().is_empty() {
                    discover_roots(&[".gemini"])
                } else {
                    vec![ScanRoot {
                        dir: PathBuf::from(home),
                        label: None,
                    }]
                }
            } else {
                discover_roots(&[".gemini"])
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

    /// Chat JSON files under `root/tmp/*/chats/*.json`.
    fn chat_files(root: &ScanRoot, max_file_size: u64) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in WalkDir::new(&root.dir)
            .max_depth(6)
            .follow_links(false)
            .into_iter()
            .flatten()
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json")
                    && path.components().any(|c| c.as_os_str() == "chats")
                {
                    if let Ok(meta) = std::fs::metadata(path) {
                        if meta.len() <= max_file_size {
                            files.push(path.to_path_buf());
                        }
                    }
                }
            }
        }
        files.sort();
        files
    }

    fn record_from_usage(
        value: &Value,
        file: &Path,
        root: &ScanRoot,
        index: usize,
    ) -> Option<UsageRecord> {
        let usage = value.get("usageMetadata").filter(|u| u.is_object())?;
        let prompt = usage
            .get("promptTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let candidates = usage
            .get("candidatesTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached = usage
            .get("cachedContentTokenCount")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        // `promptTokenCount` includes cached; keep buckets disjoint.
        let fresh_input = prompt.saturating_sub(cached);
        if fresh_input + candidates + cached == 0 {
            return None;
        }
        let model = value
            .get("model")
            .or_else(|| value.get("modelVersion"))
            .and_then(Value::as_str)
            .unwrap_or("gemini")
            .to_string();
        // Project = the chat folder immediately above the file.
        let project = file
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unknown".to_string());
        let started_at = value
            .get("timestamp")
            .and_then(Value::as_i64)
            .or_else(|| value.get("time").and_then(Value::as_i64))
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .unwrap_or_else(Utc::now);
        let session_id = file
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let rel = match &root.label {
            Some(label) => Path::new(label).join(file.strip_prefix(&root.dir).unwrap_or(file)),
            None => file.strip_prefix(&root.dir).unwrap_or(file).to_path_buf(),
        };
        Some(UsageRecord::new(
            Provider::Gemini,
            project,
            session_id,
            Usage {
                model,
                started_at,
                input_tokens: fresh_input,
                output_tokens: candidates,
                cache_read_tokens: cached,
                cache_write_tokens: 0,
                cost_micros: 0,
            },
            0,
            format!("{}:{index}", rel.display()),
        ))
    }

    /// Recursively collect usage-bearing objects from a chat JSON value.
    fn collect_usages(value: &Value, file: &Path, root: &ScanRoot, out: &mut Vec<UsageRecord>) {
        match value {
            Value::Object(map) => {
                if map.contains_key("usageMetadata") {
                    if let Some(r) = Self::record_from_usage(value, file, root, out.len()) {
                        out.push(r);
                    }
                }
                for v in map.values() {
                    Self::collect_usages(v, file, root, out);
                }
            }
            Value::Array(arr) => {
                for v in arr {
                    Self::collect_usages(v, file, root, out);
                }
            }
            _ => {}
        }
    }

    fn file_stats(path: &Path) -> (u64, i64, u64) {
        let Ok(meta) = std::fs::metadata(path) else {
            return (0, 0, 0);
        };
        let mut max_mtime = 0i64;
        if let Ok(modified) = meta.modified() {
            if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                max_mtime = unix.as_secs() as i64;
            }
        }
        (1, max_mtime, meta.len())
    }
}

impl ProviderSource for GeminiSource {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs: Vec<PathBuf> = self.existing_roots().into_iter().map(|r| r.dir).collect();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Gemini))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Gemini));
        }
        let mut errors = Vec::new();
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            for f in Self::chat_files(root, self.config.max_file_size) {
                let (cnt, m, b) = Self::file_stats(&f);
                found += cnt;
                max_mtime = max_mtime.max(m);
                total_bytes += b;
                let Ok(content) = std::fs::read_to_string(&f) else {
                    errors.push(format!("read {f:?}"));
                    continue;
                };
                let Ok(v) = serde_json::from_str::<Value>(&content) else {
                    continue;
                };
                let mut recs = Vec::new();
                Self::collect_usages(&v, &f, root, &mut recs);
                for r in recs {
                    emit(r);
                }
            }
        }
        Ok(ScanOutput {
            found_files: found,
            fingerprint: fingerprint(found, max_mtime, total_bytes),
            errors,
            ..Default::default()
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let roots = self.existing_roots();
        if roots.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Gemini));
        }
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for root in &roots {
            for f in Self::chat_files(root, self.config.max_file_size) {
                let (c, m, b) = Self::file_stats(&f);
                found += c;
                max_mtime = max_mtime.max(m);
                total_bytes += b;
            }
        }
        Ok(fingerprint(found, max_mtime, total_bytes))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use tempfile::tempdir;

    use super::*;

    fn source_for(dir: &Path) -> GeminiSource {
        GeminiSource::new(ProviderConfig {
            provider: Provider::Gemini,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    #[test]
    fn parses_usage_metadata_into_records() {
        let dir = tempdir().unwrap();
        let chats = dir.path().join("tmp").join("sess1").join("chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(
            chats.join("chat.json"),
            r#"{"model":"gemini-2.0-flash","usageMetadata":{"promptTokenCount":100,"candidatesTokenCount":20,"cachedContentTokenCount":10,"totalTokenCount":120}}"#,
        )
        .unwrap();

        let mut records = Vec::new();
        let out = source_for(dir.path())
            .scan(&mut |r| records.push(r))
            .unwrap();
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].provider, Provider::Gemini);
        assert_eq!(records[0].usage.model, "gemini-2.0-flash");
        // promptTokenCount includes cached; cache-read is split out.
        assert_eq!(records[0].usage.input_tokens, 90);
        assert_eq!(records[0].usage.output_tokens, 20);
        assert_eq!(records[0].usage.cache_read_tokens, 10);
        assert_eq!(records[0].usage.total_tokens(), 120);
    }

    #[test]
    fn skips_files_without_usage_metadata() {
        let dir = tempdir().unwrap();
        let chats = dir.path().join("tmp").join("sess2").join("chats");
        fs::create_dir_all(&chats).unwrap();
        fs::write(chats.join("plain.json"), r#"{"text":"hi"}"#).unwrap();

        let mut records = Vec::new();
        let out = source_for(dir.path())
            .scan(&mut |r| records.push(r))
            .unwrap();
        assert!(out.errors.is_empty());
        assert!(records.is_empty());
    }
}
