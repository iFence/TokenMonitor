//! Pure contribution-grid geometry and intensity mapping, shared by the GPUI
//! and TUI frontends. No rendering code here — each frontend turns these into
//! its own pixels/cells.

use chrono::{Datelike, Duration, NaiveDate};

/// Weekday rows in the GitHub-style grid (Sunday first).
pub const ROWS: i64 = 7;

/// The Sunday on or before the 365th day before `today`; the grid's first
/// column is always a full week.
pub fn grid_start(today: NaiveDate) -> NaiveDate {
    let anchor = today - Duration::days(364);
    anchor - Duration::days(anchor.weekday().num_days_from_sunday() as i64)
}

/// Number of grid columns: one per week from `start` through `today`.
pub fn week_count(start: NaiveDate, today: NaiveDate) -> usize {
    ((today - start).num_days() / 7) as usize + 1
}

/// Zero-based grid column for a date, measured from `start` (a Sunday).
pub fn week_index(date: NaiveDate, start: NaiveDate) -> usize {
    ((date - start).num_days() / 7) as usize
}

/// Map a day's token count to a 0..=4 intensity level, linear in the window's
/// maximum (mirrors the reference crate's `LinearStrategy`).
pub fn level_for(value: u64, max: u64) -> usize {
    if value == 0 {
        return 0;
    }
    if max == 0 {
        return 1;
    }
    (value * 4 / max).clamp(1, 4) as usize
}

/// `(column, "N月")` labels placed above the column containing each month's
/// first day. The partial first month (whose 1st falls before `start`) is
/// skipped, like GitHub's own grid.
pub fn month_labels(start: NaiveDate, today: NaiveDate) -> Vec<(usize, String)> {
    let mut labels = Vec::new();
    let mut year = start.year();
    let mut month = start.month();
    loop {
        let first = NaiveDate::from_ymd_opt(year, month, 1).expect("day 1 always exists");
        if first > today {
            break;
        }
        if first >= start {
            labels.push((week_index(first, start), format!("{month}")));
        }
        if month == 12 {
            year += 1;
            month = 1;
        } else {
            month += 1;
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn level_maps_linear_intensity() {
        assert_eq!(level_for(0, 100), 0);
        assert_eq!(level_for(1, 100), 1);
        assert_eq!(level_for(50, 100), 2);
        assert_eq!(level_for(100, 100), 4);
        assert_eq!(level_for(7, 0), 1);
    }

    #[test]
    fn grid_start_is_a_sunday_within_365_days() {
        // 2026-08-19 is a Wednesday; anchor = 2025-08-20 (Wednesday), so the
        // grid starts on the Sunday before: 2025-08-17. The 53-week grid can
        // span up to 370 days before today.
        let start = grid_start(day(2026, 8, 19));
        assert_eq!(start.weekday(), chrono::Weekday::Sun);
        let span = day(2026, 8, 19) - start;
        assert!(span >= Duration::days(364) && span <= Duration::days(370));
        assert_eq!(week_count(start, day(2026, 8, 19)), 53);
    }

    #[test]
    fn week_count_rounds_up_partial_weeks() {
        let start = day(2026, 8, 16); // Sunday
        assert_eq!(week_count(start, start + Duration::days(6)), 1);
        assert_eq!(week_count(start, start + Duration::days(7)), 2);
        assert_eq!(week_count(start, start + Duration::days(364)), 53);
    }

    #[test]
    fn week_index_counts_columns_from_the_start_sunday() {
        let start = day(2026, 8, 16);
        assert_eq!(week_index(start, start), 0);
        assert_eq!(week_index(start + Duration::days(7), start), 1);
        assert_eq!(week_index(start + Duration::days(20), start), 2);
    }

    #[test]
    fn month_labels_skip_partial_first_month() {
        let start = day(2026, 1, 4); // Sunday, Jan 1 is before it
        let today = day(2026, 3, 31);
        let labels = month_labels(start, today);
        assert_eq!(labels, vec![(4, "2".to_string()), (8, "3".to_string())]);
    }
}
