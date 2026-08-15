//! OpenCode data source (stub).

use std::path::PathBuf;

use crate::core::model::Provider;
use crate::core::usage::UsageRecord;

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

    fn data_dirs(&self) -> Result<Vec<PathBuf>, ProviderError> {
        if let Some(dir) = &self.config.data_dir_override {
            return Ok(vec![dir.clone()]);
        }
        // TODO: locate OpenCode data dir (e.g. ~/.local/share/opencode or ~/.opencode)
        let home = crate::platform::home_dir()
            .map_err(|_| ProviderError::DataDirNotFound(Provider::OpenCode))?;
        Ok(vec![home.join(".opencode")])
    }

    fn scan(&self, _emit: &mut dyn FnMut(UsageRecord)) -> Result<ScanOutput, ProviderError> {
        // TODO: parse OpenCode usage records
        Ok(ScanOutput::default())
    }

    fn scan_fingerprint(&self) -> Result<String, ProviderError> {
        Ok(String::new())
    }
}
