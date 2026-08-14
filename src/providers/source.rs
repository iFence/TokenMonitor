use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;
use walkdir::WalkDir;

use crate::core::model::Provider;
use crate::core::usage::UsageRecord;

/// Per-provider scan configuration.
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub provider: Provider,
    /// Override for the discovered data directory (e.g. from settings).
    pub data_dir_override: Option<PathBuf>,
    pub enabled: bool,
    /// Skip raw files larger than this many bytes.
    pub max_file_size: u64,
    /// Maximum directory depth when walking raw files.
    pub max_depth: usize,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            provider: Provider::Claude,
            data_dir_override: None,
            enabled: true,
            max_file_size: 64 * 1024 * 1024,
            max_depth: 8,
        }
    }
}

impl ProviderConfig {
    pub fn for_provider(provider: Provider) -> Self {
        ProviderConfig {
            provider,
            ..ProviderConfig::default()
        }
    }
}

/// Format the change-detection fingerprint from per-scan stats. Must stay in
/// lockstep with `dir_fingerprint` so the cheap check and a full scan agree.
pub(crate) fn fingerprint(found_files: u64, max_mtime: i64, total_bytes: u64) -> String {
    format!("{found_files}:{max_mtime}:{total_bytes}")
}

/// Cheap change detector: walk `dir` and stat each JSONL file without reading
/// it. Files skipped by a full scan (non-JSONL, oversized, unreadable metadata)
/// are excluded the same way, so an unchanged tree yields the same fingerprint
/// every call. A few ms for hundreds of files — safe to run every poll cycle.
pub(crate) fn dir_fingerprint(
    dir: &Path,
    max_depth: usize,
    max_file_size: u64,
) -> Result<String, ProviderError> {
    let mut max_mtime = 0i64;
    let mut total_bytes = 0u64;
    let mut found = 0u64;

    for entry in WalkDir::new(dir).max_depth(max_depth).follow_links(false) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        if !entry.file_type().is_file() {
            continue;
        }
        if !entry
            .path()
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("jsonl"))
        {
            continue;
        }
        let meta = match fs::metadata(entry.path()) {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() > max_file_size {
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

    Ok(fingerprint(found, max_mtime, total_bytes))
}

/// The result of scanning one provider's data directory.
#[derive(Debug, Default)]
pub struct ScanOutput {
    pub records: Vec<UsageRecord>,
    pub found_files: u64,
    /// Cheap change detector: `"<file_count>:<max_mtime_unix>:<total_bytes>"`.
    pub fingerprint: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Error)]
pub enum ProviderError {
    #[error("data directory not found for {0}")]
    DataDirNotFound(Provider),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
}

/// Contract every provider adapter implements.
pub trait ProviderSource: Send + Sync {
    fn provider(&self) -> Provider;

    /// Locate the local data directory holding raw usage files.
    fn data_dir(&self) -> Result<PathBuf, ProviderError>;

    /// Walk raw files under `data_dir()` and parse them into normalized records.
    fn scan(&self) -> Result<ScanOutput, ProviderError>;

    /// Cheap fingerprint of the source state; the scheduler skips rescan when unchanged.
    fn scan_fingerprint(&self) -> Result<String, ProviderError>;
}
