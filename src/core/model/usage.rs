use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Normalized token usage for a single AI request/response exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// Model identifier as reported by the provider, e.g. "claude-opus-4-5".
    pub model: String,
    /// When the exchange started (UTC).
    pub started_at: DateTime<Utc>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// Computed cost in USD micros (1e-6 USD).
    pub cost_micros: u64,
}

impl Usage {
    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    pub fn add(&mut self, other: &Usage) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
        self.cache_write_tokens = self
            .cache_write_tokens
            .saturating_add(other.cache_write_tokens);
        self.cost_micros = self.cost_micros.saturating_add(other.cost_micros);
    }
}
