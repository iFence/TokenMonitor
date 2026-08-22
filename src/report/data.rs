//! Loading the report's raw daily series from SQLite. Shared by the GPUI and
//! TUI frontends so both aggregate the same 365-day East-8 view.

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::Connection;

use crate::core::aggregation::SumStats;
use crate::core::model::TimeWindow;
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
