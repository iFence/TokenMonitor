use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

use crate::core::time::{east8, east8_local, east8_to_utc};

use super::quota::Period;

/// A half-open time range `[start, end)` used for aggregating usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeWindow {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub period: Period,
}

impl TimeWindow {
    /// Window covering the current period (day/week/month) up to `now`.
    pub fn current(period: Period, now: DateTime<Utc>) -> Self {
        match period {
            Period::All => TimeWindow {
                start: DateTime::<Utc>::from_timestamp(0, 0).expect("epoch"),
                end: now,
                period,
            },
            _ => {
                let start = Self::period_start(period, now);
                let end = Self::period_end(period, start);
                TimeWindow { start, end, period }
            }
        }
    }

    /// Window covering the trailing `n` East-8 calendar days ending today, with
    /// today included as a partial day up to `now`.
    pub fn last_n_days(n: i64, now: DateTime<Utc>) -> Self {
        let today_start = east8_to_utc(
            east8_local(now)
                .date_naive()
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
        );
        TimeWindow {
            start: today_start - Duration::days(n - 1),
            end: now,
            period: Period::All,
        }
    }

    pub fn contains(&self, t: DateTime<Utc>) -> bool {
        t >= self.start && t < self.end
    }

    /// Window covering the previous full period: yesterday / last week
    /// (Monday~Sunday) / last month, measured in East-8 time. Only accepts
    /// Day/Week/Month.
    pub fn previous(period: Period, now: DateTime<Utc>) -> Self {
        let end = Self::period_start(period, now); // current period start = previous period end
        let start = match period {
            Period::Day => end - Duration::days(1),
            Period::Week => end - Duration::days(7),
            Period::Month => {
                // `end` is the 1st of some month 00:00 +08; take the 1st of the month before.
                let end_local = end.with_timezone(&east8());
                let (y, m) = if end_local.month() == 1 {
                    (end_local.year() - 1, 12)
                } else {
                    (end_local.year(), end_local.month() - 1)
                };
                east8_to_utc(
                    chrono::NaiveDate::from_ymd_opt(y, m, 1)
                        .expect("first of previous month is valid")
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight is valid"),
                )
            }
            Period::All => unreachable!("All has no previous period"),
        };
        TimeWindow { start, end, period }
    }

    /// Window covering the current calendar year `[Jan 1, next Jan 1)` in
    /// East-8 time. The `period` field is set to `Period::All` as a sentinel
    /// (same as `last_n_days`).
    pub fn current_year(now: DateTime<Utc>) -> Self {
        let local = east8_local(now);
        let start = east8_to_utc(
            chrono::NaiveDate::from_ymd_opt(local.year(), 1, 1)
                .expect("jan 1 is valid")
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
        );
        let end = east8_to_utc(
            chrono::NaiveDate::from_ymd_opt(local.year() + 1, 1, 1)
                .expect("jan 1 is valid")
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
        );
        TimeWindow {
            start,
            end,
            period: Period::All,
        }
    }

    /// Window covering `[start 00:00 +08, end+1 00:00 +08)` for two East-8
    /// calendar dates (both endpoints inclusive). `start > end` is swapped so
    /// the window is never inverted.
    pub fn custom(start: NaiveDate, end: NaiveDate) -> Self {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        let start_utc = east8_to_utc(start.and_hms_opt(0, 0, 0).expect("midnight is valid"));
        let end_utc = east8_to_utc(
            (end + Duration::days(1))
                .and_hms_opt(0, 0, 0)
                .expect("midnight is valid"),
        );
        TimeWindow {
            start: start_utc,
            end: end_utc,
            period: Period::All,
        }
    }

    fn period_start(period: Period, now: DateTime<Utc>) -> DateTime<Utc> {
        let local = east8_local(now);
        let date = match period {
            Period::Week => {
                let days_from_monday = local.weekday().num_days_from_monday();
                local.date_naive() - Duration::days(days_from_monday as i64)
            }
            Period::Month => local.date_naive().with_day(1).expect("day 1 always exists"),
            _ => local.date_naive(),
        };
        east8_to_utc(date.and_hms_opt(0, 0, 0).expect("midnight is valid"))
    }

    fn period_end(period: Period, start: DateTime<Utc>) -> DateTime<Utc> {
        match period {
            Period::Day => start + Duration::days(1),
            Period::Week => start + Duration::days(7),
            Period::Month => {
                let start_local = start.with_timezone(&east8());
                let (y, m) = if start_local.month() == 12 {
                    (start_local.year() + 1, 1)
                } else {
                    (start_local.year(), start_local.month() + 1)
                };
                east8_to_utc(
                    chrono::NaiveDate::from_ymd_opt(y, m, 1)
                        .expect("first of next month is valid")
                        .and_hms_opt(0, 0, 0)
                        .expect("midnight is valid"),
                )
            }
            Period::All => unreachable!("All has no period end"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, m: u32, d: u32, h: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap()
    }

    #[test]
    fn current_day_bounds_use_east8() {
        // 2026-08-14 15:30 UTC == 2026-08-14 23:30 +08, so "today" in East 8
        // runs from 2026-08-13 16:00 UTC (midnight +08) to the next midnight.
        let w = TimeWindow::current(Period::Day, at(2026, 8, 14, 15));
        assert_eq!(w.start, at(2026, 8, 13, 16));
        assert_eq!(w.end, at(2026, 8, 14, 16));
        assert!(w.contains(at(2026, 8, 14, 15)));
        // 23:30 +08 the day before (2026-08-13 15:30 UTC) is yesterday.
        assert!(!w.contains(at(2026, 8, 13, 15)));
    }

    #[test]
    fn previous_day_is_yesterday() {
        let w = TimeWindow::previous(Period::Day, at(2026, 8, 14, 15));
        assert_eq!(w.start, at(2026, 8, 12, 16));
        assert_eq!(w.end, at(2026, 8, 13, 16));
    }

    #[test]
    fn previous_day_crosses_month() {
        let w = TimeWindow::previous(Period::Day, at(2026, 3, 1, 6));
        assert_eq!(w.start, at(2026, 2, 27, 16));
        assert_eq!(w.end, at(2026, 2, 28, 16));
    }

    #[test]
    fn previous_week_is_last_monday_to_sunday() {
        // 2026-08-14 is a Friday; this week starts Mon 2026-08-10.
        let w = TimeWindow::previous(Period::Week, at(2026, 8, 14, 12));
        assert_eq!(w.start, at(2026, 8, 2, 16));
        assert_eq!(w.end, at(2026, 8, 9, 16));
    }

    #[test]
    fn previous_week_crosses_month() {
        // 2026-06-01 is a Monday; previous week is May 25 ~ Jun 1.
        let w = TimeWindow::previous(Period::Week, at(2026, 6, 3, 9));
        assert_eq!(w.start, at(2026, 5, 24, 16));
        assert_eq!(w.end, at(2026, 5, 31, 16));
    }

    #[test]
    fn previous_month_crosses_january() {
        let w = TimeWindow::previous(Period::Month, at(2026, 1, 20, 0));
        assert_eq!(w.start, at(2025, 11, 30, 16));
        assert_eq!(w.end, at(2025, 12, 31, 16));
    }

    #[test]
    fn current_year_spans_calendar_year() {
        let w = TimeWindow::current_year(at(2026, 8, 14, 23));
        assert_eq!(w.start, at(2025, 12, 31, 16));
        assert_eq!(w.end, at(2026, 12, 31, 16));
        // 2026-12-31 23:00 UTC == 2027-01-01 07:00 +08 — the new year, excluded.
        assert!(!w.contains(at(2026, 12, 31, 23)));
        assert!(!w.contains(at(2027, 1, 1, 0)));
    }

    #[test]
    fn custom_range_uses_east8_bounds_and_swaps_inverted() {
        // 2026-08-10 ~ 2026-08-12 (inclusive) measured in East-8 calendar days.
        let w = TimeWindow::custom(
            NaiveDate::from_ymd_opt(2026, 8, 10).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
        );
        assert_eq!(w.start, at(2026, 8, 9, 16)); // 2026-08-10 00:00 +08
        assert_eq!(w.end, at(2026, 8, 12, 16)); // 2026-08-13 00:00 +08 (exclusive)
        assert!(w.contains(at(2026, 8, 12, 15))); // 23:00 +08 on the last day
        assert!(!w.contains(at(2026, 8, 12, 16))); // next day, excluded

        // Inverted inputs are swapped, not left as an empty window.
        let swapped = TimeWindow::custom(
            NaiveDate::from_ymd_opt(2026, 8, 12).unwrap(),
            NaiveDate::from_ymd_opt(2026, 8, 10).unwrap(),
        );
        assert_eq!(swapped, w);
    }

    #[test]
    fn last_n_days_spans_exactly_n_calendar_days() {
        // 2026-08-14 15:00 UTC == 2026-08-14 23:00 +08, so "today" is Aug 14.
        let w = TimeWindow::last_n_days(7, at(2026, 8, 14, 15));
        // 7 days ending today: Aug 8 ..= Aug 14 in East-8 time.
        assert_eq!(w.start, at(2026, 8, 7, 16)); // Aug 8 00:00 +08
        assert_eq!(w.end, at(2026, 8, 14, 15)); // now (partial today)
    }
}
