//! Embedded pricing data: OpenRouter base table + local overrides + built-in
//! defaults, merged with tokei's three-level precedence
//! (`pricing_overrides.json` > `pricing.json` > built-in defaults).

use std::collections::HashMap;

use serde::Deserialize;

use super::pricer::Price;

/// Built-in fallback prices (USD per 1M tokens), mirroring tokei's
/// `_DEFAULT_PRICES`. Seeds the merged table and acts as an offline safety net
/// should the embedded JSON be missing or empty.
const DEFAULT_PRICES: &[(&str, Price)] = &[
    (
        "anthropic/claude-opus-4.8",
        Price {
            input: 5.0,
            output: 25.0,
            cache_read: 0.5,
            cache_write: 6.25,
        },
    ),
    (
        "anthropic/claude-sonnet-4.6",
        Price {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        },
    ),
    (
        "anthropic/claude-haiku-4.5",
        Price {
            input: 1.0,
            output: 5.0,
            cache_read: 0.1,
            cache_write: 1.25,
        },
    ),
    (
        "openai/gpt-5.5",
        Price {
            input: 5.0,
            output: 30.0,
            cache_read: 0.5,
            cache_write: 0.0,
        },
    ),
    (
        "qwen/qwen3.7-max",
        Price {
            input: 1.25,
            output: 3.75,
            cache_read: 0.25,
            cache_write: 1.5625,
        },
    ),
    (
        "deepseek/deepseek-v4-pro",
        Price {
            input: 0.435,
            output: 0.87,
            cache_read: 0.0036,
            cache_write: 0.0,
        },
    ),
    (
        "google/gemini-3.5-flash",
        Price {
            input: 1.5,
            output: 9.0,
            cache_read: 0.15,
            cache_write: 0.0833,
        },
    ),
    (
        "google/gemini-3.1-pro-preview",
        Price {
            input: 2.0,
            output: 12.0,
            cache_read: 0.2,
            cache_write: 0.375,
        },
    ),
    (
        "x-ai/grok-4.5",
        Price {
            input: 2.0,
            output: 6.0,
            cache_read: 0.3,
            cache_write: 0.0,
        },
    ),
    (
        "tencent/hy3",
        Price {
            input: 0.14,
            output: 0.58,
            cache_read: 0.035,
            cache_write: 0.0,
        },
    ),
    (
        "tencent/hy3-preview",
        Price {
            input: 0.063,
            output: 0.21,
            cache_read: 0.021,
            cache_write: 0.0,
        },
    ),
];

/// `pricing.json` top level: `{ "_meta": ..., "models": { "<id>": {in,out,...} } }`.
#[derive(Deserialize)]
struct PricingFile {
    models: HashMap<String, Price>,
}

/// `pricing_overrides.json` top level: `models` (price overrides) + `aliases`.
#[derive(Deserialize)]
struct OverridesFile {
    #[serde(default)]
    models: HashMap<String, Price>,
    #[serde(default)]
    aliases: HashMap<String, String>,
}

/// Merge order: defaults → `pricing.json` → `pricing_overrides.json` models.
pub(super) fn build_models() -> HashMap<String, Price> {
    let mut models: HashMap<String, Price> = DEFAULT_PRICES
        .iter()
        .map(|(k, v)| ((*k).to_string(), *v))
        .collect();
    if let Ok(file) = serde_json::from_str::<PricingFile>(include_str!("pricing.json")) {
        models.extend(file.models);
    }
    if let Ok(file) = serde_json::from_str::<OverridesFile>(include_str!("pricing_overrides.json"))
    {
        models.extend(file.models);
    }
    models
}

/// Alias table (`local model name → canonical id`) from `pricing_overrides.json`.
pub(super) fn build_aliases() -> HashMap<String, String> {
    serde_json::from_str::<OverridesFile>(include_str!("pricing_overrides.json"))
        .map(|f| f.aliases)
        .unwrap_or_default()
}
