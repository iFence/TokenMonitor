use crate::core::model::{ModelPricing, Usage};

/// Compute the cost (USD micros) of a usage record against model pricing.
///
/// Returns 0 when the record's model does not match the pricing entry — the
/// caller decides which pricing applies by looking up the exact model.
pub fn compute_cost_micros(pricing: &ModelPricing, usage: &Usage) -> u64 {
    if pricing.model != usage.model {
        return 0;
    }
    pricing.cost_micros(
        usage.input_tokens,
        usage.output_tokens,
        usage.cache_read_tokens,
        usage.cache_write_tokens,
    )
}
