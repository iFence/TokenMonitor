//! Qwen Code data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;

use super::source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

pub struct QwenSource {
    config: ProviderConfig,
}

impl QwenSource {
    pub fn new(config: ProviderConfig) -> Self {
        QwenSource { config }
    }
}

impl ProviderSource for QwenSource {
    fn provider(&self) -> Provider {
        Provider::Qwen
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        // TODO: locate Qwen Code data dir
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Qwen))?;
        Ok(home.join(".qwen"))
    }

    fn scan(&self) -> Result<ScanOutput, ProviderError> {
        // TODO: parse Qwen Code usage records
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
