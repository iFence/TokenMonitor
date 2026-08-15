//! Domain-layer tests exercising the public API of `rtoken::core`.

use chrono::{Duration, TimeZone, Utc};

use rtoken::aggregation::{by_day, by_model, by_project, by_provider, total};
use rtoken::model::{Period, Provider, TimeWindow, Usage};
use rtoken::pricing::Pricer;
use rtoken::quota::QuotaTracker;
use rtoken::usage::UsageRecord;

fn record(
    provider: Provider,
    project: &str,
    model: &str,
    day_offset: i64,
    tokens: u64,
) -> UsageRecord {
    UsageRecord::new(
        provider,
        project.to_string(),
        "session".to_string(),
        Usage {
            model: model.to_string(),
            started_at: Utc::now() - Duration::days(day_offset),
            input_tokens: tokens,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost_micros: tokens,
        },
        0,
        format!("fp-{day_offset}-{model}-{tokens}"),
    )
}

#[test]
fn total_sums_all_records() {
    let records = vec![
        record(Provider::Claude, "a", "m1", 0, 10),
        record(Provider::Codex, "a", "m2", 1, 20),
    ];
    let t = total(&records);
    assert_eq!(t.records, 2);
    assert_eq!(t.input_tokens, 30);
    assert_eq!(t.cost_micros, 30);
}

#[test]
fn by_provider_groups_and_sorts_by_cost_desc() {
    let records = vec![
        record(Provider::Claude, "a", "m", 0, 10),
        record(Provider::Codex, "a", "m", 0, 20),
    ];
    let rows = by_provider(&records);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, Provider::Codex); // higher cost first
}

#[test]
fn by_project_and_by_day_and_by_model_group() {
    let records = vec![
        record(Provider::Claude, "proj-a", "claude-opus", 0, 10),
        record(Provider::Claude, "proj-b", "claude-opus", 1, 5),
    ];
    let projects = by_project(&records);
    assert_eq!(projects.len(), 2);
    assert!(projects.iter().any(|(name, _)| name == "proj-a"));

    let days = by_day(&records);
    assert_eq!(days.len(), 2); // two distinct days

    let models = by_model(&records);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].0, "claude-opus");
}

#[test]
fn pricer_computes_cost_for_known_model_and_zero_for_unknown() {
    // 1M input tokens on claude-opus-4.8 at $5/Mtok = $5 = 5_000_000 micros.
    let cost =
        Pricer::global().cost_micros(Provider::Claude, "claude-opus-4-8", 1_000_000, 0, 0, 0);
    assert_eq!(cost, 5_000_000);

    // Empty model name has no price, so its cost is 0.
    assert_eq!(
        Pricer::global().cost_micros(Provider::Claude, "", 1_000_000, 0, 0, 0),
        0
    );
}

#[test]
fn quota_refresh_sums_only_matching_provider_and_window() {
    let now = Utc::now();
    let mut tracker = QuotaTracker::new();
    tracker.set(Provider::Claude, Period::Day, 100_000, 0);

    let records = vec![
        record(Provider::Claude, "p", "m", 0, 30),
        record(Provider::Codex, "p", "m", 0, 999),
    ];
    tracker.refresh(&records, now);
    let claude = tracker
        .quotas()
        .iter()
        .find(|q| q.provider == Provider::Claude)
        .expect("claude quota");
    assert_eq!(claude.used_tokens, 30);
    assert_eq!(claude.used_cost_micros, 30);
}

#[test]
fn time_window_bounds() {
    // 2026-08-14 12:00 UTC == 2026-08-14 20:00 East 8; day boundaries are +08.
    let now = Utc.with_ymd_and_hms(2026, 8, 14, 12, 0, 0).unwrap();
    let day = TimeWindow::current(Period::Day, now);
    assert_eq!(
        day.start,
        Utc.with_ymd_and_hms(2026, 8, 13, 16, 0, 0).unwrap()
    );
    assert!(day.contains(now));
    assert!(!day.contains(day.start - Duration::seconds(1)));

    let trailing = TimeWindow::last_n_days(7, now);
    assert_eq!(
        trailing.start,
        Utc.with_ymd_and_hms(2026, 8, 6, 16, 0, 0).unwrap()
    );
    // Half-open window [start, now): a point strictly inside is contained.
    assert!(trailing.contains(now - Duration::minutes(5)));
    assert!(!trailing.contains(trailing.start - Duration::seconds(1)));
}
