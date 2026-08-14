use serde::{Deserialize, Serialize};

use crate::core::usage::UsageRecord;

/// Aggregated token/cost stats over a set of usage records.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SumStats {
    pub records: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_micros: u64,
}

impl SumStats {
    /// Stats for a single record.
    pub fn from_record(r: &UsageRecord) -> Self {
        SumStats {
            records: 1,
            input_tokens: r.usage.input_tokens,
            output_tokens: r.usage.output_tokens,
            cache_read_tokens: r.usage.cache_read_tokens,
            cache_write_tokens: r.usage.cache_write_tokens,
            cost_micros: r.usage.cost_micros,
        }
    }

    pub fn total_tokens(&self) -> u64 {
        self.input_tokens
            .saturating_add(self.output_tokens)
            .saturating_add(self.cache_read_tokens)
            .saturating_add(self.cache_write_tokens)
    }

    pub fn add(&mut self, other: &SumStats) {
        self.records = self.records.saturating_add(other.records);
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

    pub fn merge(&mut self, other: SumStats) {
        self.add(&other);
    }
}
