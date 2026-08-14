use chrono::{DateTime, Utc};

use crate::core::model::{Period, Provider, Quota, TimeWindow};
use crate::core::usage::UsageRecord;

/// Computes quota usage from a set of usage records within each quota's window.
#[derive(Debug, Clone, Default)]
pub struct QuotaTracker {
    quotas: Vec<Quota>,
}

impl QuotaTracker {
    pub fn new() -> Self {
        QuotaTracker { quotas: Vec::new() }
    }

    /// Set a quota's limits for a provider + period, merging if it already exists.
    pub fn set(
        &mut self,
        provider: Provider,
        period: Period,
        limit_tokens: u64,
        limit_cost_micros: u64,
    ) {
        if let Some(q) = self
            .quotas
            .iter_mut()
            .find(|q| q.provider == provider && q.period == period)
        {
            q.limit_tokens = limit_tokens;
            q.limit_cost_micros = limit_cost_micros;
            return;
        }
        self.quotas.push(Quota {
            provider,
            period,
            limit_tokens,
            used_tokens: 0,
            limit_cost_micros,
            used_cost_micros: 0,
            updated_at: Utc::now(),
        });
    }

    /// Recompute `used_*` for every quota from records falling in each quota's window.
    pub fn refresh(&mut self, records: &[UsageRecord], now: DateTime<Utc>) {
        for quota in &mut self.quotas {
            let window = TimeWindow::current(quota.period, now);
            let mut used_tokens = 0u64;
            let mut used_cost_micros = 0u64;
            for r in records {
                if r.provider == quota.provider && window.contains(r.usage.started_at) {
                    used_tokens += r.usage.total_tokens();
                    used_cost_micros += r.usage.cost_micros;
                }
            }
            quota.used_tokens = used_tokens;
            quota.used_cost_micros = used_cost_micros;
            quota.updated_at = now;
        }
    }

    pub fn quotas(&self) -> &[Quota] {
        &self.quotas
    }
}
