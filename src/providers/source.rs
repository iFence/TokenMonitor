use std::path::PathBuf;

use thiserror::Error;

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
