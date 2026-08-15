//! Pricing lookup and provider-aware cost computation, mirroring tokei's cost
//! model (CALCULATION.md §4): three-level price lookup, model-name
//! normalization with a conservative fallback, and per-provider cost formulas.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::core::model::Provider;

use super::data::{build_aliases, build_models};
use super::normalize::{normalize, FAMILY};

/// Per-model price in USD per 1,000,000 tokens.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct Price {
    #[serde(rename = "in")]
    pub input: f64,
    #[serde(rename = "out")]
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

/// Bump to force a one-time re-cost of existing rows when the embedded pricing
/// data changes (see `Collector::open` backfill guard).
pub const PRICING_VERSION: &str = "1";
pub const PRICING_VERSION_KEY: &str = "pricing.version";

/// Codex high-context surcharge threshold, in raw input tokens. Codex's raw
/// `input_tokens` includes the cached portion, which rToken stores as
/// `input_tokens + cache_read_tokens`.
const CODEX_HIGH_CONTEXT_TOKENS: u64 = 272_000;

/// Immutable pricing table: merged model prices + aliases, built once.
pub struct Pricer {
    models: HashMap<String, Price>,
    aliases: HashMap<String, String>,
}

impl Pricer {
    /// Process-wide singleton; the embedded data is static and immutable.
    pub fn global() -> &'static Pricer {
        static PRICER: LazyLock<Pricer> = LazyLock::new(|| Pricer {
            models: build_models(),
            aliases: build_aliases(),
        });
        &PRICER
    }

    /// Resolve a local model name to its price via tokei's fallback chain
    /// (alias → normalize → gemini pro/flash → family keyword → provider
    /// fallback). Returns `None` only for empty / `<synthetic>` models.
    pub fn resolve(&self, model: &str, provider: Provider) -> Option<&Price> {
        let id = self.resolve_id(model, provider)?;
        self.models.get(&id)
    }

    /// Compute cost in USD micros (1e-6 USD) for one request's token buckets.
    ///
    /// The "per 1M tokens" price and "USD micros" denominators cancel out, so
    /// `cost_micros = Σ tokens × usd_per_mtok`.
    pub fn cost_micros(
        &self,
        provider: Provider,
        model: &str,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
    ) -> u64 {
        let Some(price) = self.resolve(model, provider) else {
            return 0;
        };
        let micros = if provider == Provider::Codex {
            // Codex surcharges high-context requests and does not bill cache
            // writes (tokei ignores them).
            let high = input.saturating_add(cache_read) > CODEX_HIGH_CONTEXT_TOKENS;
            let p_in = price.input * if high { 2.0 } else { 1.0 };
            let p_cr = price.cache_read * if high { 2.0 } else { 1.0 };
            let p_out = price.output * if high { 1.5 } else { 1.0 };
            input as f64 * p_in + cache_read as f64 * p_cr + output as f64 * p_out
        } else {
            input as f64 * price.input
                + output as f64 * price.output
                + cache_read as f64 * price.cache_read
                + cache_write as f64 * price.cache_write
        };
        micros.round().max(0.0) as u64
    }

    /// Full resolution to a canonical id (may be a fallback representative).
    fn resolve_id(&self, model: &str, provider: Provider) -> Option<String> {
        let s = model.trim();
        if s.is_empty() || s.eq_ignore_ascii_case("<synthetic>") {
            return None;
        }
        if let Some(alias) = self.aliases.get(s) {
            return Some(alias.clone());
        }
        let norm = normalize(s)?;
        if self.models.contains_key(&norm) {
            return Some(norm);
        }
        let low = s.to_lowercase();
        if low.contains("gemini") {
            return Some(if low.contains("pro") {
                "google/gemini-3.1-pro-preview".to_string()
            } else {
                "google/gemini-3.5-flash".to_string()
            });
        }
        for (kw, rep) in FAMILY {
            if low.contains(kw) {
                return Some((*rep).to_string());
            }
        }
        // Conservative fallback for unknown models: Codex → gpt-5.5, others → opus.
        Some(match provider {
            Provider::Codex => "openai/gpt-5.5".to_string(),
            _ => "anthropic/claude-opus-4.8".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pricer() -> &'static Pricer {
        Pricer::global()
    }

    #[test]
    fn claude_linear_cost() {
        // claude-opus-4.8: in 5.0 / out 25.0 / cache_read 0.5 / cache_write 6.25.
        // 1M input + 0.5M cache_write = 5.0 + 3.125 = $8.125 = 8_125_000 micros.
        let cost = pricer().cost_micros(
            Provider::Claude,
            "claude-opus-4-8",
            1_000_000,
            0,
            0,
            500_000,
        );
        assert_eq!(cost, 8_125_000);
    }

    #[test]
    fn codex_no_surcharge_below_threshold() {
        // gpt-5.5: in 5.0 / out 30.0 / cache_read 0.5. Non-cached input 200k.
        // 200k in + 50k cache_read + 10k out = 1.0 + 0.025 + 0.3 = $1.325.
        let cost = pricer().cost_micros(Provider::Codex, "gpt-5.5", 200_000, 10_000, 50_000, 0);
        assert_eq!(cost, 1_325_000);
    }

    #[test]
    fn codex_high_context_surcharge() {
        // input + cache_read = 300k > 272k → in×2, cache_read×2, out×1.5.
        // 200k in ×10 + 100k cache_read ×1.0 + 10k out ×45 = 2.0 + 0.1 + 0.45.
        let cost = pricer().cost_micros(Provider::Codex, "gpt-5.5", 200_000, 10_000, 100_000, 0);
        assert_eq!(cost, 2_550_000);
    }

    #[test]
    fn codex_ignores_cache_write() {
        // cache_write is not billed for Codex, matching tokei.
        let without = pricer().cost_micros(Provider::Codex, "gpt-5.5", 1_000, 1_000, 0, 0);
        let with = pricer().cost_micros(Provider::Codex, "gpt-5.5", 1_000, 1_000, 0, 999_999);
        assert_eq!(without, with);
    }

    #[test]
    fn empty_model_costs_zero() {
        assert_eq!(
            pricer().cost_micros(Provider::Claude, "", 1000, 1000, 0, 0),
            0
        );
        assert_eq!(
            pricer().cost_micros(Provider::Codex, "  ", 1000, 1000, 0, 0),
            0
        );
        assert_eq!(
            pricer().cost_micros(Provider::Claude, "<synthetic>", 1000, 1000, 0, 0),
            0
        );
    }

    #[test]
    fn unknown_model_falls_back_per_provider() {
        // Unknown Claude model → opus; unknown Codex model → gpt-5.5.
        assert_eq!(
            pricer()
                .resolve("totally-unknown", Provider::Claude)
                .map(|p| p.input),
            Some(5.0) // claude-opus-4.8 in
        );
        assert_eq!(
            pricer()
                .resolve("totally-unknown", Provider::Codex)
                .map(|p| p.input),
            Some(5.0) // gpt-5.5 in (coincidentally also 5.0)
        );
        // gpt-5 family keyword routes a not-yet-listed model to gpt-5.5.
        assert_eq!(
            pricer()
                .resolve("gpt-5.6-sol", Provider::Codex)
                .map(|p| p.output),
            Some(30.0)
        );
    }

    #[test]
    fn alias_routes_to_canonical_price() {
        // pricing_overrides.json aliases gemini-3-pro-preview → google/gemini-3.1-pro-preview.
        let price = pricer()
            .resolve("gemini-3-pro-preview", Provider::Gemini)
            .unwrap();
        assert_eq!(price.input, 2.0);
    }
}
