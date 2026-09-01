//! Trae data source: parses tokscale's synced Trae cache.
//!
//! Trae encrypts its local conversation/usage database, so token counts are
//! not readable from Trae's own files. Mirroring how the reference project
//! (Javis603/token-monitor, via tokscale) surfaces Trae numbers, this adapter
//! reads the per-account usage dump that `tokscale trae sync` writes under
//! `trae-cache/sessions/*.json` in the tokscale config root — `%APPDATA%/tokscale`
//! on Windows, `$XDG_CONFIG_HOME/tokscale` on Linux/macOS, or the
//! `TOKSCALE_CONFIG_DIR` override. The cache is a JSON array of session-level
//! records whose token buckets are already exact, so each cached session
//! becomes one [`UsageRecord`].

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use walkdir::WalkDir;

use crate::core::model::{Provider, Usage};
use crate::core::usage::UsageRecord;

use super::source::{fingerprint, ProviderConfig, ProviderError, ProviderSource, ScanOutput};

/// The cache is account-level and names no workspace, so all records land under
/// one project. Keeps the "by project" view tidy while the per-session id
/// preserves the granularity the API provides.
const PROJECT: &str = "Trae";

/// One cached Trae session. `usage_time` is Unix epoch *seconds* (the usage API
/// returns seconds, not milliseconds). Fields are optional so a malformed
/// record is skipped rather than failing the whole file.
#[derive(Deserialize)]
struct TraeSession {
    #[serde(default)]
    model_name: String,
    #[serde(default)]
    mode: String,
    session_id: Option<String>,
    usage_time: Option<i64>,
    #[serde(default)]
    extra_info: ExtraInfo,
}

/// Token buckets as returned by the usage API. Trae's "Auto" mode still reports
/// exact input/output figures, so no per-turn bucketing is needed here.
#[derive(Deserialize, Default)]
struct ExtraInfo {
    #[serde(default)]
    input_token: i64,
    #[serde(default)]
    output_token: i64,
    #[serde(default)]
    cache_read_token: i64,
    #[serde(default)]
    cache_write_token: i64,
}

/// Known Trae display names → canonical (tiktoken-style) model ids, so
/// downstream pricing — which keys on canonical ids — can route them. Unknown
/// names pass through lowercased. Mirrors tokscale's `normalize_trae_model`.
fn normalize_trae_model(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let normalized: &str = match lower.as_str() {
        "gpt-5.4" => "gpt-5.4",
        "gpt-5.3" | "gpt-5.3 codex" | "gpt-5.3-codex" => "gpt-5.3-codex",
        "gpt-5.2" | "gpt-5.2 codex" | "gpt-5.2-codex" => "gpt-5.2-codex",
        "gpt-5.1" | "gpt-5.1 codex" | "gpt-5.1-codex" => "gpt-5.1-codex",
        "gemini 3.1 pro" => "gemini-3.1-pro",
        "gemini 3.1" => "gemini-3.1",
        "glm 5.1" | "glm-5.1" => "glm-5.1",
        "claude sonnet 4.6" | "claude-sonnet-4.6" => "claude-sonnet-4.6",
        "claude sonnet 4.5" | "claude-sonnet-4.5" => "claude-sonnet-4.5",
        other => other,
    };
    normalized.to_string()
}

pub struct TraeSource {
    config: ProviderConfig,
    /// Candidate cache directories, resolved lazily on the first scan (scan
    /// thread) so the directory probe never blocks UI startup.
    dirs: OnceLock<Vec<PathBuf>>,
}

impl TraeSource {
    pub fn new(config: ProviderConfig) -> Self {
        TraeSource {
            config,
            dirs: OnceLock::new(),
        }
    }

    fn cache_dir_candidates(&self) -> Vec<PathBuf> {
        if let Some(dir) = &self.config.data_dir_override {
            return vec![dir.clone()];
        }
        let mut dirs = Vec::new();
        if let Some(root) = std::env::var_os("TOKSCALE_CONFIG_DIR") {
            dirs.push(PathBuf::from(root).join("trae-cache").join("sessions"));
        }
        if let Some(config_dir) = dirs::config_dir() {
            dirs.push(
                config_dir
                    .join("tokscale")
                    .join("trae-cache")
                    .join("sessions"),
            );
        }
        dirs
    }

    fn dirs(&self) -> &[PathBuf] {
        self.dirs.get_or_init(|| self.cache_dir_candidates())
    }

    fn existing_dirs(&self) -> Vec<PathBuf> {
        self.dirs().iter().filter(|d| d.is_dir()).cloned().collect()
    }

    fn is_cache_file(path: &Path) -> bool {
        path.extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("json"))
    }

    /// Parse one JSON-array cache file, emitting a record per session.
    fn parse_file(
        path: &Path,
        rel: &Path,
        emit: &mut dyn FnMut(UsageRecord),
    ) -> Result<(), String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read {:?}: {e}", path))?;
        let sessions: Vec<TraeSession> =
            serde_json::from_slice(&bytes).map_err(|e| format!("parse {:?}: {e}", path))?;
        for session in &sessions {
            if let Some(r) = Self::record(session, rel) {
                emit(r);
            }
        }
        Ok(())
    }

    /// Convert one cached session into a normalized record. Records without a
    /// real `session_id` or a positive `usage_time` are dropped: they cannot be
    /// deduplicated or timestamped correctly.
    fn record(session: &TraeSession, rel: &Path) -> Option<UsageRecord> {
        let session_id = session.session_id.as_deref().unwrap_or("");
        let usage_time = session.usage_time?;
        if session_id.is_empty() || usage_time <= 0 {
            return None;
        }

        let input = session.extra_info.input_token.max(0) as u64;
        let output = session.extra_info.output_token.max(0) as u64;
        let cache_read = session.extra_info.cache_read_token.max(0) as u64;
        let cache_write = session.extra_info.cache_write_token.max(0) as u64;
        if input + output + cache_read + cache_write == 0 {
            return None;
        }

        let model = if !session.model_name.trim().is_empty() {
            normalize_trae_model(&session.model_name)
        } else if !session.mode.trim().is_empty() {
            format!("trae-{}", session.mode.trim().to_ascii_lowercase())
        } else {
            "trae-unknown".to_string()
        };

        // `usage_time` is epoch seconds; the API never emits milliseconds, so
        // feed it straight into `DateTime::from_timestamp`.
        let started_at = DateTime::<Utc>::from_timestamp(usage_time, 0)?;

        Some(UsageRecord::new(
            Provider::Trae,
            PROJECT.to_string(),
            session_id.to_string(),
            Usage {
                model,
                started_at,
                input_tokens: input,
                output_tokens: output,
                cache_read_tokens: cache_read,
                cache_write_tokens: cache_write,
                cost_micros: 0, // pricing applied in a later pipeline stage
            },
            0,
            format!("{}:{session_id}:{usage_time}", rel.display()),
        ))
    }
}

impl ProviderSource for TraeSource {
    fn provider(&self) -> Provider {
        Provider::Trae
    }

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        let dirs = self.existing_dirs();
        if dirs.is_empty() {
            Err(ProviderError::DataDirNotFound(Provider::Trae))
        } else {
            Ok(dirs)
        }
    }

    fn scan(&self, emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        let dirs = self.existing_dirs();
        if dirs.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Trae));
        }
        let mut errors = Vec::new();
        let mut found_files = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;

        for dir in &dirs {
            for entry in WalkDir::new(dir)
                .max_depth(self.config.max_depth)
                .follow_links(false)
            {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        errors.push(format!("walk error: {e}"));
                        continue;
                    }
                };
                if !entry.file_type().is_file() || !Self::is_cache_file(entry.path()) {
                    continue;
                }
                let path = entry.path();
                let meta = match std::fs::metadata(path) {
                    Ok(m) => m,
                    Err(e) => {
                        errors.push(format!("metadata {path:?}: {e}"));
                        continue;
                    }
                };
                if meta.len() > self.config.max_file_size {
                    continue;
                }
                found_files += 1;
                total_bytes += meta.len();
                if let Ok(modified) = meta.modified() {
                    if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                        max_mtime = max_mtime.max(unix.as_secs() as i64);
                    }
                }
                let rel = path.strip_prefix(dir).unwrap_or(path);
                if let Err(e) = Self::parse_file(path, rel, emit) {
                    errors.push(e);
                }
            }
        }

        Ok(ScanOutput {
            found_files,
            fingerprint: fingerprint(found_files, max_mtime, total_bytes),
            errors,
            ..Default::default()
        })
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        let dirs = self.existing_dirs();
        if dirs.is_empty() {
            return Err(ProviderError::DataDirNotFound(Provider::Trae));
        }
        let mut found = 0u64;
        let mut max_mtime = 0i64;
        let mut total_bytes = 0u64;
        for dir in &dirs {
            for entry in WalkDir::new(dir)
                .max_depth(self.config.max_depth)
                .follow_links(false)
            {
                let Ok(entry) = entry else { continue };
                if !entry.file_type().is_file() || !Self::is_cache_file(entry.path()) {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(entry.path()) else {
                    continue;
                };
                if meta.len() > self.config.max_file_size {
                    continue;
                }
                found += 1;
                total_bytes += meta.len();
                if let Ok(modified) = meta.modified() {
                    if let Ok(unix) = modified.duration_since(std::time::UNIX_EPOCH) {
                        max_mtime = max_mtime.max(unix.as_secs() as i64);
                    }
                }
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

    fn source_for(dir: &Path) -> TraeSource {
        TraeSource::new(ProviderConfig {
            provider: Provider::Trae,
            data_dir_override: Some(dir.to_path_buf()),
            ..ProviderConfig::default()
        })
    }

    fn scan_collect(src: &TraeSource) -> (ScanOutput, Vec<UsageRecord>) {
        let mut records = Vec::new();
        let out = src.scan(&mut |r| records.push(r)).unwrap();
        (out, records)
    }

    #[test]
    fn parses_synced_cache_into_records() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("sessions.json"),
            r#"[{
                "model_name": "GPT-5.4",
                "session_id": "test-session-1",
                "usage_time": 1776000000,
                "dollar_float": 0.5,
                "extra_info": {
                    "input_token": 1000,
                    "output_token": 500,
                    "cache_read_token": 200,
                    "cache_write_token": 100
                }
            }]"#,
        )
        .unwrap();

        let (out, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(out.found_files, 1);
        assert!(out.errors.is_empty());
        assert_eq!(records.len(), 1);

        let r = &records[0];
        assert_eq!(r.provider, Provider::Trae);
        assert_eq!(r.project, "Trae");
        assert_eq!(r.session_id, "test-session-1");
        assert_eq!(r.usage.model, "gpt-5.4");
        assert_eq!(
            r.usage.started_at,
            DateTime::<Utc>::from_timestamp(1776000000, 0).unwrap()
        );
        assert_eq!(r.usage.input_tokens, 1000);
        assert_eq!(r.usage.output_tokens, 500);
        assert_eq!(r.usage.cache_read_tokens, 200);
        assert_eq!(r.usage.cache_write_tokens, 100);
        assert_eq!(r.usage.total_tokens(), 1800);
        assert!(r.fingerprint.ends_with(":test-session-1:1776000000"));
    }

    #[test]
    fn model_name_takes_priority_and_mode_is_fallback() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("auto.json"),
            r#"[{
                "model_name": "",
                "mode": "Auto",
                "session_id": "auto-session",
                "usage_time": 1776000000,
                "dollar_float": 0.27,
                "extra_info": {
                    "input_token": 159213,
                    "output_token": 210,
                    "cache_read_token": 6144,
                    "cache_write_token": 0
                }
            }]"#,
        )
        .unwrap();

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.model, "trae-auto");
    }

    #[test]
    fn normalizes_recognized_and_unknown_models() {
        assert_eq!(normalize_trae_model("GPT-5.4"), "gpt-5.4");
        assert_eq!(normalize_trae_model("GPT-5.3 Codex"), "gpt-5.3-codex");
        assert_eq!(normalize_trae_model("Gemini 3.1 Pro"), "gemini-3.1-pro");
        assert_eq!(
            normalize_trae_model("Claude Sonnet 4.6"),
            "claude-sonnet-4.6"
        );
        assert_eq!(normalize_trae_model("GLM-5.1"), "glm-5.1");
        assert_eq!(normalize_trae_model("SomeOtherModel"), "someothermodel");
    }

    #[test]
    fn skips_malformed_or_zero_usage_records() {
        let dir = tempdir().unwrap();
        // A record with an empty session id (no stable dedup), a zero usage
        // time (would land at epoch), and a zero-token session are all dropped.
        fs::write(
            dir.path().join("sessions.json"),
            r#"[{
                "model_name": "GPT-5.4",
                "session_id": "",
                "usage_time": 1776000000,
                "dollar_float": 0.1,
                "extra_info": {"input_token": 100, "output_token": 1, "cache_read_token": 0, "cache_write_token": 0}
            }, {
                "model_name": "GPT-5.4",
                "session_id": "no-time",
                "usage_time": 0,
                "dollar_float": 0.1,
                "extra_info": {"input_token": 100, "output_token": 1, "cache_read_token": 0, "cache_write_token": 0}
            }, {
                "model_name": "GPT-5.4",
                "session_id": "empty",
                "usage_time": 1776000000,
                "dollar_float": 0.0,
                "extra_info": {"input_token": 0, "output_token": 0, "cache_read_token": 0, "cache_write_token": 0}
            }]"#,
        )
        .unwrap();

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert!(records.is_empty());
    }

    #[test]
    fn negative_buckets_are_clamped_and_never_fail_the_file() {
        let dir = tempdir().unwrap();
        fs::write(
            dir.path().join("sessions.json"),
            r#"[{
                "model_name": "GPT-5.4",
                "session_id": "neg",
                "usage_time": 1776000000,
                "dollar_float": 0.1,
                "extra_info": {"input_token": -5, "output_token": 10, "cache_read_token": -2, "cache_write_token": 0}
            }]"#,
        )
        .unwrap();

        let (_, records) = scan_collect(&source_for(dir.path()));
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage.input_tokens, 0);
        assert_eq!(records[0].usage.output_tokens, 10);
        assert_eq!(records[0].usage.cache_read_tokens, 0);
    }

    #[test]
    fn data_dir_missing_is_data_dir_not_found() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("no-such-dir");
        let err = source_for(&missing).data_dirs().unwrap_err();
        assert!(matches!(
            err,
            ProviderError::DataDirNotFound(Provider::Trae)
        ));
    }

    #[test]
    fn fingerprint_changes_when_cache_rewritten() {
        let dir = tempdir().unwrap();
        let src = source_for(dir.path());
        fs::write(
            dir.path().join("sessions.json"),
            r#"[{"model_name":"GPT-5.4","session_id":"s1","usage_time":1776000000,"extra_info":{"input_token":1,"output_token":1,"cache_read_token":0,"cache_write_token":0}}]"#,
        )
        .unwrap();
        let fp1 = src.scan_fingerprint().unwrap();
        let fp2 = src.scan_fingerprint().unwrap();
        assert_eq!(fp1, fp2);

        std::thread::sleep(std::time::Duration::from_secs(2));
        fs::write(
            dir.path().join("sessions.json"),
            r#"[{"model_name":"GPT-5.4","session_id":"s2","usage_time":1776000000,"extra_info":{"input_token":2,"output_token":2,"cache_read_token":0,"cache_write_token":0}}]"#,
        )
        .unwrap();
        assert_ne!(fp1, src.scan_fingerprint().unwrap());
    }
}
