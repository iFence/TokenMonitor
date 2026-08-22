//! Loading the report's raw daily series from SQLite. Shared by the GPUI and
//! TUI frontends so both aggregate the same 365-day East-8 view.

use std::collections::BTreeMap;

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::Connection;

use crate::core::aggregation::SumStats;
use crate::core::model::{Provider, TimeWindow};
use crate::storage::repository::UsageRepo;

/// East-8 calendar days with recorded usage inside `window`, chronological
/// ascending. Days without usage are omitted — callers overlay them onto the
/// grid themselves.
pub fn load_report_days(
    conn: &Connection,
    window: &TimeWindow,
) -> Result<Vec<(NaiveDate, SumStats)>> {
    let days: Vec<(NaiveDate, SumStats)> = UsageRepo::new(conn)
        .daily_series(window)?
        .into_iter()
        .filter_map(|(key, stats)| {
            NaiveDate::parse_from_str(&key, "%Y-%m-%d")
                .ok()
                .map(|date| (date, stats))
        })
        .collect();
    Ok(days)
}

/// Per-provider ("agent") aggregates over `window`, sorted by token usage
/// descending.
pub fn load_report_by_provider(
    conn: &Connection,
    window: &TimeWindow,
) -> Result<Vec<(Provider, SumStats)>> {
    let mut v = UsageRepo::new(conn).aggregate_by_provider(window)?;
    v.sort_by(|a, b| b.1.total_tokens().cmp(&a.1.total_tokens()));
    Ok(v)
}

/// Per-model aggregates across all providers over `window`, sorted by token
/// usage descending.
pub fn load_report_by_model(
    conn: &Connection,
    window: &TimeWindow,
) -> Result<Vec<(String, SumStats)>> {
    let by_provider_model = UsageRepo::new(conn).aggregate_by_provider_model(window)?;
    let mut by_model: BTreeMap<String, SumStats> = BTreeMap::new();
    for models in by_provider_model.values() {
        for (model, stats) in models {
            by_model.entry(model.clone()).or_default().add(stats);
        }
    }
    let mut v: Vec<(String, SumStats)> = by_model.into_iter().collect();
    v.sort_by(|a, b| b.1.total_tokens().cmp(&a.1.total_tokens()));
    Ok(v)
}
