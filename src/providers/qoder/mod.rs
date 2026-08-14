//! Qoder data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;

use super::source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

pub struct QoderSource {
    config: ProviderConfig,
}

impl QoderSource {
    pub fn new(config: ProviderConfig) -> Self {
        QoderSource { config }
    }
}

impl ProviderSource for QoderSource {
    fn provider(&self) -> Provider {
        Provider::Qoder
    }

    fn data_dir(&self) -> Result<PathBuf, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(dir.clone());
        }
        // TODO: locate Qoder data dir
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Qoder))?;
        Ok(home.join(".qoder"))
    }

    fn scan(&self) -> Result<ScanOutput, ProviderError> {
        // TODO: parse Qoder usage records
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
