//! Gemini CLI data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;

use super::source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

pub struct GeminiSource {
    config: ProviderConfig,
}

impl GeminiSource {
    pub fn new(config: ProviderConfig) -> Self {
        GeminiSource { config }
    }
}

impl ProviderSource for GeminiSource {
    fn provider(&self) -> Provider {
        Provider::Gemini
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        // TODO: locate Gemini CLI data dir (e.g. ~/.gemini)
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Gemini))?;
        Ok(home.join(".gemini"))
    }

    fn scan(&self) -> Result<ScanOutput, ProviderError> {
        // TODO: parse Gemini CLI usage records
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
