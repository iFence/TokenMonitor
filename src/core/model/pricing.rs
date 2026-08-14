use serde::{Deserialize, Serialize};

use super::provider::Provider;

/// Per-model pricing in USD per 1,000,000 tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPricing {
    pub provider: Provider,
    pub model: String,
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    pub cache_read_usd_per_mtok: f64,
    pub cache_write_usd_per_mtok: f64,
}

impl ModelPricing {
    /// Compute cost in USD micros (1e-6 USD) for the given token counts.
    ///
    /// `cost_micros = Σ tokens * usd_per_mtok`: the "per 1e6 tokens" and
    /// "USD micros" denominators cancel out.
    pub fn cost_micros(&self, input: u64, output: u64, cache_read: u64, cache_write: u64) -> u64 {
        let micros = input as f64 * self.input_usd_per_mtok
            + output as f64 * self.output_usd_per_mtok
            + cache_read as f64 * self.cache_read_usd_per_mtok
            + cache_write as f64 * self.cache_write_usd_per_mtok;
        micros.round().max(0.0) as u64
    }
}
