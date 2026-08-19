//! Pure summary statistics for the report page.
//!
//! No GPUI imports here: everything is plain date arithmetic over the daily
//! series, so the streak/busiest-day logic stays unit-testable.

use std::collections::BTreeMap;

use chrono::{Duration, NaiveDate};

use crate::core::aggregation::SumStats;

/// Summary metrics derived from the report's daily series.
#[derive(Debug, Clone, Default)]
pub struct ReportStats {
    pub total: SumStats,
    /// Days (within the window) that have at least one recorded usage row.
    pub active_days: u32,
    /// Longest run of consecutive active calendar days in the window.
    pub longest_streak: u32,
    /// Consecutive active days ending today (or yesterday if today is still
    /// empty), so a fresh start of the day does not zero the streak.
    pub current_streak: u32,
    /// Day with the most tokens, broken by cost when tied.
    pub busiest: Option<(NaiveDate, SumStats)>,
}

/// Compute report stats over `days` — East-8 calendar days with usage, sorted
/// chronologically — for the trailing 365-day window ending at `today`
/// (East-8, inclusive).
pub fn report_stats(days: &[(NaiveDate, SumStats)], today: NaiveDate) -> ReportStats {
    let window_start = today - Duration::days(364);

    let mut total = SumStats::default();
    let mut by_day: BTreeMap<NaiveDate, SumStats> = BTreeMap::new();
    let mut busiest: Option<(NaiveDate, SumStats)> = None;
    for &(date, stats) in days {
        if date < window_start || date > today {
            continue;
        }
        total.add(&stats);
        by_day.insert(date, stats);
        let replaces = match &busiest {
            None => true,
            Some((_, best)) => {
                stats.total_tokens() > best.total_tokens()
                    || (stats.total_tokens() == best.total_tokens()
                        && stats.cost_micros > best.cost_micros)
            }
        };
        if replaces {
            busiest = Some((date, stats));
        }
    }

    ReportStats {
        total,
        active_days: by_day.len() as u32,
        longest_streak: longest_streak(&by_day, window_start, today),
        current_streak: current_streak(&by_day, today, window_start),
        busiest,
    }
}

/// Longest run of consecutive dates present in `by_day` over `[start, end]`.
fn longest_streak(by_day: &BTreeMap<NaiveDate, SumStats>, start: NaiveDate, end: NaiveDate) -> u32 {
    let mut longest = 0u32;
    let mut run = 0u32;
    let mut cursor = start;
    while cursor <= end {
        if by_day.contains_key(&cursor) {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
        cursor += Duration::days(1);
    }
    longest
}

/// Consecutive active days ending at `today`. If today has no usage yet, the
/// run starts from yesterday so an untouched morning does not hide an ongoing
/// streak.
fn current_streak(
    by_day: &BTreeMap<NaiveDate, SumStats>,
    today: NaiveDate,
    window_start: NaiveDate,
) -> u32 {
    let mut cursor = today;
    if !by_day.contains_key(&cursor) {
        cursor -= Duration::days(1);
    }
    let mut streak = 0u32;
    while by_day.contains_key(&cursor) {
        streak += 1;
        if cursor == window_start {
            break;
        }
        cursor -= Duration::days(1);
    }
    streak
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    fn stats(tokens: u64, cost_micros: u64) -> SumStats {
        SumStats {
            records: 1,
            input_tokens: tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micros,
        }
    }

    #[test]
    fn totals_and_active_days_are_summed() {
        let today = day(2026, 8, 19);
        let days = vec![
            (day(2026, 8, 17), stats(100, 10)),
            (day(2026, 8, 18), stats(200, 20)),
        ];
        let s = report_stats(&days, today);
        assert_eq!(s.total.total_tokens(), 300);
        assert_eq!(s.total.cost_micros, 30);
        assert_eq!(s.active_days, 2);
    }

    #[test]
    fn longest_streak_counts_consecutive_days_only() {
        let today = day(2026, 8, 19);
        // Active: 8/10-8/11, 8/14-8/18 (5-day run), 8/19 alone.
        let days = vec![
            (day(2026, 8, 10), stats(1, 1)),
            (day(2026, 8, 11), stats(1, 1)),
            (day(2026, 8, 14), stats(1, 1)),
            (day(2026, 8, 15), stats(1, 1)),
            (day(2026, 8, 16), stats(1, 1)),
            (day(2026, 8, 17), stats(1, 1)),
            (day(2026, 8, 18), stats(1, 1)),
            (day(2026, 8, 19), stats(1, 1)),
        ];
        let s = report_stats(&days, today);
        assert_eq!(s.longest_streak, 6);
        assert_eq!(s.current_streak, 6);
    }

    #[test]
    fn current_streak_survives_an_empty_today() {
        let today = day(2026, 8, 19);
        // Today has no rows yet; the 8/16-8/18 run should still count.
        let days = vec![
            (day(2026, 8, 16), stats(1, 1)),
            (day(2026, 8, 17), stats(1, 1)),
            (day(2026, 8, 18), stats(1, 1)),
        ];
        let s = report_stats(&days, today);
        assert_eq!(s.current_streak, 3);
        assert_eq!(s.active_days, 3);
    }

    #[test]
    fn current_streak_breaks_on_a_gap() {
        let today = day(2026, 8, 19);
        // 8/17 and 8/18 active, 8/16 inactive.
        let days = vec![
            (day(2026, 8, 17), stats(1, 1)),
            (day(2026, 8, 18), stats(1, 1)),
        ];
        let s = report_stats(&days, today);
        assert_eq!(s.current_streak, 2);
    }

    #[test]
    fn busiest_day_prefers_more_tokens_then_more_cost() {
        let today = day(2026, 8, 19);
        let days = vec![
            (day(2026, 8, 17), stats(500, 50)),
            (day(2026, 8, 18), stats(900, 40)),
        ];
        let s = report_stats(&days, today);
        assert_eq!(s.busiest, Some((day(2026, 8, 18), stats(900, 40))));
    }

    #[test]
    fn out_of_window_days_are_ignored() {
        let today = day(2026, 8, 19);
        let days = vec![
            (day(2025, 8, 20), stats(999, 999)), // 364 days back: kept
            (day(2025, 8, 19), stats(999, 999)), // 365 days back: dropped
            (day(2026, 8, 20), stats(999, 999)), // future: dropped
        ];
        let s = report_stats(&days, today);
        assert_eq!(s.active_days, 1);
        assert_eq!(s.total.total_tokens(), 999);
    }
}
