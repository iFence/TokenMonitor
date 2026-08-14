//! OpenCode data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;

use super::source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

pub struct OpenCodeSource {
    config: ProviderConfig,
}

impl OpenCodeSource {
    pub fn new(config: ProviderConfig) -> Self {
        OpenCodeSource { config }
    }
}

impl ProviderSource for OpenCodeSource {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        // TODO: locate OpenCode data dir (e.g. ~/.local/share/opencode or ~/.opencode)
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::OpenCode))?;
        Ok(home.join(".opencode"))
    }

    fn scan(&self) -> Result<ScanOutput, ProviderError> {
        // TODO: parse OpenCode usage records
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
