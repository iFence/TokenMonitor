//! Timezone policy for the whole app: UTC+08:00 (East 8 / Asia/Shanghai).
//!
//! Day/week/month boundaries and displayed timestamps are computed in this
//! fixed offset (no DST), matching the intended deployment locale. Instants are
//! still stored as UTC — only bucketing and formatting use East 8.

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};

/// UTC+08:00 — the only timezone the app displays and buckets by.
pub fn east8() -> FixedOffset {
    FixedOffset::east_opt(8 * 3600).expect("UTC+08:00 is a valid fixed offset")
}

/// `now` expressed in East-8 wall-clock time.
pub fn east8_local(now: DateTime<Utc>) -> DateTime<FixedOffset> {
    now.with_timezone(&east8())
}

/// Interpret a naive local datetime as East-8 wall-clock and return the UTC
/// instant it refers to. The shared definition of "start of a calendar day".
pub fn east8_to_utc(naive: NaiveDateTime) -> DateTime<Utc> {
    east8()
        .from_local_datetime(&naive)
        .single()
        .expect("fixed-offset local time is never ambiguous")
        .with_timezone(&Utc)
}
