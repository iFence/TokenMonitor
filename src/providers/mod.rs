//! Provider data-source adapters: one per AI coding tool.

pub mod claude;
pub mod codex;
pub mod gemini;
pub mod opencode;
pub mod qoder;
pub mod qwen;
mod source;

pub use source::{ProviderConfig, ProviderError, ProviderSource, ScanOutput};

use crate::core::model::Provider;

/// All providers rToken can track, in display order.
pub fn all_providers() -> [Provider; 6] {
    Provider::ALL
}

/// Default scan configs for every provider.
pub fn default_configs() -> Vec<ProviderConfig> {
    Provider::ALL
        .into_iter()
        .map(ProviderConfig::for_provider)
        .collect()
}

/// Build source adapters for the enabled configs.
pub fn build_sources(configs: &[ProviderConfig]) -> Vec<Box<dyn ProviderSource>> {
    configs
        .iter()
        .filter(|c| c.enabled)
        .map(|c| match c.provider {
            Provider::Claude => {
                Box::new(claude::ClaudeSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Codex => {
                Box::new(codex::CodexSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Gemini => {
                Box::new(gemini::GeminiSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Qwen => Box::new(qwen::QwenSource::new(c.clone())) as Box<dyn ProviderSource>,
            Provider::OpenCode => {
                Box::new(opencode::OpenCodeSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Qoder => {
                Box::new(qoder::QoderSource::new(c.clone())) as Box<dyn ProviderSource>
            }
        })
        .collect()
}
