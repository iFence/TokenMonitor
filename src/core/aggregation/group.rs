use std::collections::BTreeMap;

use crate::core::model::Provider;
use crate::core::usage::UsageRecord;

use super::sum_stats::SumStats;
use super::window::{day_key, month_key, week_key};

/// Sum all records into a single aggregate.
pub fn total(records: &[UsageRecord]) -> SumStats {
    records.iter().fold(SumStats::default(), |mut acc, r| {
        acc.add(&SumStats::from_record(r));
        acc
    })
}

/// Per-provider aggregates, sorted by cost descending (tokei-style).
pub fn by_provider(records: &[UsageRecord]) -> Vec<(Provider, SumStats)> {
    let mut map: BTreeMap<Provider, SumStats> = BTreeMap::new();
    for r in records {
        map.entry(r.provider)
            .or_default()
            .add(&SumStats::from_record(r));
    }
    sorted_by_cost_desc(map)
}

/// Per-project aggregates, sorted by cost descending.
pub fn by_project(records: &[UsageRecord]) -> Vec<(String, SumStats)> {
    let mut map: BTreeMap<String, SumStats> = BTreeMap::new();
    for r in records {
        map.entry(r.project.clone())
            .or_default()
            .add(&SumStats::from_record(r));
    }
    sorted_by_cost_desc(map)
}

/// Per-day aggregates (ISO `YYYY-MM-DD`), sorted by cost descending.
pub fn by_day(records: &[UsageRecord]) -> Vec<(String, SumStats)> {
    let mut map: BTreeMap<String, SumStats> = BTreeMap::new();
    for r in records {
        map.entry(day_key(r.usage.started_at))
            .or_default()
            .add(&SumStats::from_record(r));
    }
    sorted_by_cost_desc(map)
}

/// Per-week aggregates (ISO `YYYY-Www`), sorted by cost descending.
pub fn by_week(records: &[UsageRecord]) -> Vec<(String, SumStats)> {
    let mut map: BTreeMap<String, SumStats> = BTreeMap::new();
    for r in records {
        map.entry(week_key(r.usage.started_at))
            .or_default()
            .add(&SumStats::from_record(r));
    }
    sorted_by_cost_desc(map)
}

/// Per-month aggregates (ISO `YYYY-MM`), sorted by cost descending.
pub fn by_month(records: &[UsageRecord]) -> Vec<(String, SumStats)> {
    let mut map: BTreeMap<String, SumStats> = BTreeMap::new();
    for r in records {
        map.entry(month_key(r.usage.started_at))
            .or_default()
            .add(&SumStats::from_record(r));
    }
    sorted_by_cost_desc(map)
}

/// Per-model aggregates (by model name), sorted by cost descending.
pub fn by_model(records: &[UsageRecord]) -> Vec<(String, SumStats)> {
    let mut map: BTreeMap<String, SumStats> = BTreeMap::new();
    for r in records {
        map.entry(r.usage.model.clone())
            .or_default()
            .add(&SumStats::from_record(r));
    }
    sorted_by_cost_desc(map)
}

/// Per-provider per-model aggregates (each provider's models sorted by cost
/// descending), for the dashboard card "by model" expansion.
pub fn by_provider_model(records: &[UsageRecord]) -> BTreeMap<Provider, Vec<(String, SumStats)>> {
    let mut map: BTreeMap<Provider, BTreeMap<String, SumStats>> = BTreeMap::new();
    for r in records {
        map.entry(r.provider)
            .or_default()
            .entry(r.usage.model.clone())
            .or_default()
            .add(&SumStats::from_record(r));
    }
    map.into_iter()
        .map(|(p, models)| (p, sorted_by_cost_desc(models)))
        .collect()
}

fn sorted_by_cost_desc<K: Ord>(map: BTreeMap<K, SumStats>) -> Vec<(K, SumStats)> {
    let mut v: Vec<_> = map.into_iter().collect();
    v.sort_by(|a, b| b.1.cost_micros.cmp(&a.1.cost_micros));
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::Usage;
    use chrono::Utc;

    fn record(provider: Provider, model: &str, cost: u64) -> UsageRecord {
        UsageRecord::new(
            provider,
            "proj".to_string(),
            "session".to_string(),
            Usage {
                model: model.to_string(),
                started_at: Utc::now(),
                input_tokens: cost,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_micros: cost,
            },
            0,
            format!("fp-{provider:?}-{model}-{cost}"),
        )
    }

    #[test]
    fn by_provider_model_groups_per_provider_and_sorts_models_by_cost() {
        let records = vec![
            record(Provider::Claude, "sonnet", 10),
            record(Provider::Claude, "opus", 30),
            record(Provider::Claude, "sonnet", 5),
            record(Provider::Codex, "gpt", 20),
        ];
        let map = by_provider_model(&records);
        assert_eq!(map.len(), 2);

        let claude = &map[&Provider::Claude];
        assert_eq!(claude.len(), 2);
        assert_eq!(claude[0].0, "opus"); // higher cost first
        assert_eq!(claude[0].1.cost_micros, 30);
        assert_eq!(claude[1].0, "sonnet");
        assert_eq!(claude[1].1.cost_micros, 15); // merged
        assert_eq!(claude[1].1.records, 2);

        let codex = &map[&Provider::Codex];
        assert_eq!(codex.len(), 1);
        assert_eq!(codex[0].0, "gpt");
    }

    #[test]
    fn by_provider_model_empty_records() {
        assert!(by_provider_model(&[]).is_empty());
    }
}
