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

/// Per-hour aggregates (index 0..24, East-8) of `day`, hours without usage
/// zero-filled. Backs the TUI's per-hour "today" chart and hourly panel.
pub fn load_report_hours(conn: &Connection, day: NaiveDate) -> Result<[SumStats; 24]> {
    let mut hours: [SumStats; 24] = std::array::from_fn(|_| SumStats::default());
    for (h, stats) in UsageRepo::new(conn).stats_by_hour(&day.format("%Y-%m-%d").to_string())? {
        if let Some(slot) = hours.get_mut(h as usize) {
            *slot = stats;
        }
    }
    Ok(hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Provider, Usage};
    use crate::core::time::east8_to_utc;
    use crate::core::usage::UsageRecord;
    use crate::storage::sqlite;
    use chrono::{NaiveDate, NaiveDateTime};

    /// A record at `east8_hour`:hh:15 on `day` (East-8 wall clock), converted to
    /// the UTC instant it refers to and stored as RFC3339. `seq` keeps
    /// fingerprints unique so same-hour records are not deduped away.
    fn record(day: &str, east8_hour: u32, tokens: u64, seq: u32) -> UsageRecord {
        let started = NaiveDateTime::parse_from_str(
            &format!("{day} {east8_hour:02}:15:00"),
            "%Y-%m-%d %H:%M:%S",
        )
        .unwrap();
        UsageRecord::new(
            Provider::from_id("codex").unwrap(),
            "demo".to_string(),
            "s".to_string(),
            Usage {
                model: "test".to_string(),
                started_at: east8_to_utc(started),
                input_tokens: tokens,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost_micros: 0,
            },
            0,
            format!("{day}-{east8_hour}-{seq}"),
        )
    }

    #[test]
    fn buckets_today_by_east8_hour() {
        let conn = Connection::open_in_memory().unwrap();
        sqlite::init_schema(&conn).unwrap();
        let repo = UsageRepo::new(&conn);
        repo.batch_insert_dedup(&[
            record("2026-08-22", 8, 100, 1),
            record("2026-08-22", 8, 200, 2), // same East-8 hour, summed
            record("2026-08-22", 14, 50, 3),
            record("2026-08-22", 23, 30, 4),
            record("2026-08-21", 23, 999, 5), // previous East-8 day, out of scope
        ])
        .unwrap();

        let hours =
            load_report_hours(&conn, NaiveDate::from_ymd_opt(2026, 8, 22).unwrap()).unwrap();
        assert_eq!(hours[8].total_tokens(), 300);
        assert_eq!(hours[14].total_tokens(), 50);
        assert_eq!(hours[23].total_tokens(), 30);
        assert_eq!(hours[0].total_tokens(), 0);
        assert_eq!(hours.iter().map(|s| s.total_tokens()).sum::<u64>(), 380);
        assert_eq!(hours[8].records, 2);
    }
}
