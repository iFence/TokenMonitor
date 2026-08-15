//! Qoder data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;
use crate::core::usage::UsageRecord;

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

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(vec![dir.clone()]);
        }
        // TODO: locate Qoder data dir
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::Qoder))?;
        Ok(vec![home.join(".qoder")])
    }

    fn scan(&self, _emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        // TODO: parse Qoder usage records
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
