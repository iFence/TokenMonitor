use chrono::{DateTime, Utc};

use crate::core::time::east8;

/// Bucket-key helpers — return the same key for all timestamps in one bucket.
///
/// Keys are derived from the East-8 calendar date (not UTC), so a day's
/// records land under the date the user sees. ISO-8601 string keys sort
/// chronologically under byte ordering, so a `BTreeMap` (or the raw strings)
/// can be used directly for ordered output.

/// `YYYY-MM-DD`
pub fn day_key(t: DateTime<Utc>) -> String {
    t.with_timezone(&east8()).format("%Y-%m-%d").to_string()
}

/// ISO week `YYYY-Www`, e.g. `2026-W33`.
pub fn week_key(t: DateTime<Utc>) -> String {
    t.with_timezone(&east8()).format("%G-W%V").to_string()
}

/// `YYYY-MM`
pub fn month_key(t: DateTime<Utc>) -> String {
    t.with_timezone(&east8()).format("%Y-%m").to_string()
}
