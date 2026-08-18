//! Provider data-source adapters: one per AI coding tool.

pub mod antigravity;
pub mod claude;
pub mod codebuddy;
pub mod codex;
pub mod deepseek;
pub mod gemini;
pub mod openclaw;
pub mod opencode;
pub mod pi;
pub mod qoder;
mod roots;
mod source;
pub mod workbuddy;

pub use source::{FileStates, ProviderConfig, ProviderError, ProviderSource, ScanOutput};

use crate::core::model::{Provider, ProviderSelection};

/// All providers rToken can track, in display order.
pub fn all_providers() -> [Provider; 11] {
    Provider::ALL
}

/// Default scan configs for every provider.
pub fn default_configs() -> Vec<ProviderConfig> {
    Provider::ALL
        .into_iter()
        .map(ProviderConfig::for_provider)
        .collect()
}

/// Build scan configs from the user's provider selection (order + enabled
/// flags).
pub fn configs_for(selection: &ProviderSelection) -> Vec<ProviderConfig> {
    selection
        .entries
        .iter()
        .map(|e| ProviderConfig {
            provider: e.provider,
            enabled: e.enabled,
            ..ProviderConfig::default()
        })
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
            Provider::Antigravity => {
                Box::new(antigravity::AntigravitySource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Codebuddy => {
                Box::new(codebuddy::CodebuddySource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Workbuddy => {
                Box::new(workbuddy::WorkbuddySource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::OpenCode => {
                Box::new(opencode::OpenCodeSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Qoder => {
                Box::new(qoder::QoderSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::OpenClaw => {
                Box::new(openclaw::OpenClawSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::DeepSeek => {
                Box::new(deepseek::DeepSeekSource::new(c.clone())) as Box<dyn ProviderSource>
            }
            Provider::Pi => Box::new(pi::PiSource::new(c.clone())) as Box<dyn ProviderSource>,
        })
        .collect()
}
