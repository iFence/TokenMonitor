//! Codex CLI data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;

use super::source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

pub struct CodexSource {
    config: ProviderConfig,
}

impl CodexSource {
    pub fn new(config: ProviderConfig) -> Self {
        CodexSource { config }
    }
}

impl ProviderSource for CodexSource {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        // TODO: locate Codex CLI data dir (e.g. ~/.codex/sessions)
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Codex))?;
        Ok(home.join(".codex"))
    }

    fn scan(&self) -> Result<ScanOutput, ProviderError> {
        // TODO: parse Codex session JSON into UsageRecords
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
