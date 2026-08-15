use std::collections::BTreeMap;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::core::aggregation::SumStats;
use crate::core::model::{Provider, TimeWindow, Usage};
use crate::core::usage::UsageRecord;

/// Outcome of a dedup batch insert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BatchInsertStats {
    pub inserted: u64,
    pub skipped_duplicates: u64,
}

pub struct UsageRepo<'a> {
    conn: &'a Connection,
}

impl<'a> UsageRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        UsageRepo { conn }
    }

    pub fn insert(&self, r: &UsageRecord) -> Result<i64> {
        self.conn.execute(
            "INSERT INTO usage_records
             (provider, project, session_id, model, started_at,
              input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
              cost_micros, raw_bytes, fingerprint)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                r.provider.id(),
                r.project,
                r.session_id,
                r.usage.model,
                r.usage.started_at.to_rfc3339(),
                r.usage.input_tokens as i64,
                r.usage.output_tokens as i64,
                r.usage.cache_read_tokens as i64,
                r.usage.cache_write_tokens as i64,
                r.usage.cost_micros as i64,
                r.raw_bytes as i64,
                r.fingerprint,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Batch insert, ignoring rows whose fingerprint already exists.
    pub fn batch_insert_dedup(&self, records: &[UsageRecord]) -> Result<BatchInsertStats> {
        let mut stats = BatchInsertStats::default();
        if records.is_empty() {
            return Ok(stats);
        }
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO usage_records
                 (provider, project, session_id, model, started_at,
                  input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                  cost_micros, raw_bytes, fingerprint)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            )?;
            for r in records {
                let affected = stmt.execute(params![
                    r.provider.id(),
                    r.project,
                    r.session_id,
                    r.usage.model,
                    r.usage.started_at.to_rfc3339(),
                    r.usage.input_tokens as i64,
                    r.usage.output_tokens as i64,
                    r.usage.cache_read_tokens as i64,
                    r.usage.cache_write_tokens as i64,
                    r.usage.cost_micros as i64,
                    r.raw_bytes as i64,
                    r.fingerprint,
                ])?;
                if affected > 0 {
                    stats.inserted += 1;
                } else {
                    stats.skipped_duplicates += 1;
                }
            }
        }
        tx.commit()?;
        Ok(stats)
    }

    pub fn find_by_fingerprint(&self, fingerprint: &str) -> Result<Option<UsageRecord>> {
        self.conn
            .query_row(
                "SELECT * FROM usage_records WHERE fingerprint = ?1",
                params![fingerprint],
                record_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn query_by_window(&self, w: &TimeWindow) -> Result<Vec<UsageRecord>> {
        self.query_where(
            "started_at >= ?1 AND started_at < ?2",
            params![w.start.to_rfc3339(), w.end.to_rfc3339()],
        )
    }

    pub fn query_by_provider(&self, p: Provider, w: &TimeWindow) -> Result<Vec<UsageRecord>> {
        self.query_where(
            "provider = ?1 AND started_at >= ?2 AND started_at < ?3",
            params![p.id(), w.start.to_rfc3339(), w.end.to_rfc3339()],
        )
    }

    pub fn query_by_project(&self, project: &str, w: &TimeWindow) -> Result<Vec<UsageRecord>> {
        self.query_where(
            "project = ?1 AND started_at >= ?2 AND started_at < ?3",
            params![project, w.start.to_rfc3339(), w.end.to_rfc3339()],
        )
    }

    /// Aggregate token/cost totals over a time window.
    pub fn aggregate_window(&self, w: &TimeWindow) -> Result<SumStats> {
        self.conn
            .query_row(
                "SELECT COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_write_tokens), 0),
                    COALESCE(SUM(cost_micros), 0)
             FROM usage_records
             WHERE started_at >= ?1 AND started_at < ?2",
                params![w.start.to_rfc3339(), w.end.to_rfc3339()],
                |row| {
                    Ok(SumStats {
                        records: row.get::<_, i64>(0)? as u64,
                        input_tokens: row.get::<_, i64>(1)? as u64,
                        output_tokens: row.get::<_, i64>(2)? as u64,
                        cache_read_tokens: row.get::<_, i64>(3)? as u64,
                        cache_write_tokens: row.get::<_, i64>(4)? as u64,
                        cost_micros: row.get::<_, i64>(5)? as u64,
                    })
                },
            )
            .map_err(Into::into)
    }

    /// Per-provider aggregates over a window, sorted by cost descending.
    pub fn aggregate_by_provider(&self, w: &TimeWindow) -> Result<Vec<(Provider, SumStats)>> {
        self.aggregate_grouped("provider", w)?
            .into_iter()
            .map(|(id, s)| {
                let provider = Provider::from_id(&id).ok_or_else(|| sqlite_error(&id))?;
                Ok((provider, s))
            })
            .collect()
    }

    /// Per-provider per-model aggregates over a window; each provider's models
    /// sorted by cost descending.
    pub fn aggregate_by_provider_model(
        &self,
        w: &TimeWindow,
    ) -> Result<BTreeMap<Provider, Vec<(String, SumStats)>>> {
        let sql = format!(
            "SELECT provider, model,
                COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COALESCE(SUM(cache_write_tokens), 0),
                COALESCE(SUM(cost_micros), 0)
             FROM usage_records
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY provider, model"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![w.start.to_rfc3339(), w.end.to_rfc3339()], |row| {
            let provider_id: String = row.get(0)?;
            let model: String = row.get(1)?;
            let stats = sum_stats_from_row_at(row, 2)?;
            Ok((provider_id, model, stats))
        })?;
        let mut map: BTreeMap<Provider, Vec<(String, SumStats)>> = BTreeMap::new();
        for r in rows {
            let (provider_id, model, stats) = r?;
            let provider =
                Provider::from_id(&provider_id).ok_or_else(|| sqlite_error(&provider_id))?;
            map.entry(provider).or_default().push((model, stats));
        }
        for models in map.values_mut() {
            models.sort_by(|a, b| b.1.cost_micros.cmp(&a.1.cost_micros));
        }
        Ok(map)
    }

    /// Per-project aggregates over a window, sorted by cost descending.
    pub fn aggregate_by_project(&self, w: &TimeWindow) -> Result<Vec<(String, SumStats)>> {
        self.aggregate_grouped("project", w)
    }

    /// Per-calendar-day aggregates (East-8 wall-clock day, `YYYY-MM-DD`),
    /// sorted by cost descending. `date(started_at, '+8 hours')` reproduces
    /// `core::aggregation::window::day_key`.
    pub fn aggregate_by_day(&self, w: &TimeWindow) -> Result<Vec<(String, SumStats)>> {
        self.aggregate_grouped("date(started_at, '+8 hours')", w)
    }

    /// Daily series over `w` (East-8 `YYYY-MM-DD` day key), in chronological
    /// ascending order — the shape consumed by time-series charts.
    pub fn daily_series(&self, w: &TimeWindow) -> Result<Vec<(String, SumStats)>> {
        self.daily_series_where(None, w)
    }

    /// Per-provider daily series over `w`, chronological ascending.
    pub fn daily_series_by_provider(
        &self,
        p: Provider,
        w: &TimeWindow,
    ) -> Result<Vec<(String, SumStats)>> {
        self.daily_series_where(Some(("provider", p.id())), w)
    }

    /// Per-model per-day series for one provider, chronological ascending,
    /// keyed by model id.
    pub fn daily_series_by_provider_model(
        &self,
        p: Provider,
        w: &TimeWindow,
    ) -> Result<BTreeMap<String, Vec<(String, SumStats)>>> {
        let sql = "SELECT model, date(started_at, '+8 hours'),
                        COUNT(*),
                        COALESCE(SUM(input_tokens), 0),
                        COALESCE(SUM(output_tokens), 0),
                        COALESCE(SUM(cache_read_tokens), 0),
                        COALESCE(SUM(cache_write_tokens), 0),
                        COALESCE(SUM(cost_micros), 0)
                   FROM usage_records
                   WHERE provider = ?1 AND started_at >= ?2 AND started_at < ?3
                   GROUP BY model, date(started_at, '+8 hours')
                   ORDER BY model, date(started_at, '+8 hours')";
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![p.id(), w.start.to_rfc3339(), w.end.to_rfc3339()], |row| {
            let model: String = row.get(0)?;
            let day: String = row.get(1)?;
            let stats = sum_stats_from_row_at(row, 2)?;
            Ok((model, day, stats))
        })?;
        let mut map: BTreeMap<String, Vec<(String, SumStats)>> = BTreeMap::new();
        for r in rows {
            let (model, day, stats) = r?;
            map.entry(model).or_default().push((day, stats));
        }
        Ok(map)
    }

    /// Shared chronological daily-series aggregate, optionally filtered by one
    /// equality column (e.g. `("provider", "claude")`).
    fn daily_series_where(
        &self,
        filter: Option<(&str, &str)>,
        w: &TimeWindow,
    ) -> Result<Vec<(String, SumStats)>> {
        let (where_sql, params): (String, Vec<String>) = match filter {
            Some((col, val)) => (
                format!("{col} = ? AND started_at >= ? AND started_at < ?"),
                vec![val.to_string(), w.start.to_rfc3339(), w.end.to_rfc3339()],
            ),
            None => (
                "started_at >= ? AND started_at < ?".to_string(),
                vec![w.start.to_rfc3339(), w.end.to_rfc3339()],
            ),
        };
        let sql = format!(
            "SELECT date(started_at, '+8 hours'),
                    COUNT(*),
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COALESCE(SUM(cache_write_tokens), 0),
                    COALESCE(SUM(cost_micros), 0)
             FROM usage_records
             WHERE {where_sql}
             GROUP BY date(started_at, '+8 hours')
             ORDER BY date(started_at, '+8 hours')"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |row| {
            let key: String = row.get(0)?;
            Ok((key, sum_stats_from_row(row)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Shared `GROUP BY <group_sql>` aggregate; returns keys + stats sorted by
    /// cost descending. Column 0 is the group key, columns 1..=6 are SumStats.
    fn aggregate_grouped(
        &self,
        group_sql: &str,
        w: &TimeWindow,
    ) -> Result<Vec<(String, SumStats)>> {
        let sql = format!(
            "SELECT {group_sql},
                COUNT(*),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COALESCE(SUM(cache_write_tokens), 0),
                COALESCE(SUM(cost_micros), 0)
             FROM usage_records
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY {group_sql}"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![w.start.to_rfc3339(), w.end.to_rfc3339()], |row| {
            let key: String = row.get(0)?;
            let stats = sum_stats_from_row(row)?;
            Ok((key, stats))
        })?;
        let mut v: Vec<_> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
        v.sort_by(|a, b| b.1.cost_micros.cmp(&a.1.cost_micros));
        Ok(v)
    }

    pub fn distinct_projects(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT project FROM usage_records ORDER BY project")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    fn query_where(
        &self,
        where_sql: &str,
        params: impl rusqlite::Params,
    ) -> Result<Vec<UsageRecord>> {
        let sql = format!("SELECT * FROM usage_records WHERE {where_sql} ORDER BY started_at");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params, record_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

/// Read a SumStats from a GROUP BY row whose columns `start..start+6` hold the
/// six stats (COUNT, input, output, cache_read, cache_write, cost).
fn sum_stats_from_row_at(row: &Row<'_>, start: usize) -> rusqlite::Result<SumStats> {
    Ok(SumStats {
        records: row.get::<_, i64>(start)? as u64,
        input_tokens: row.get::<_, i64>(start + 1)? as u64,
        output_tokens: row.get::<_, i64>(start + 2)? as u64,
        cache_read_tokens: row.get::<_, i64>(start + 3)? as u64,
        cache_write_tokens: row.get::<_, i64>(start + 4)? as u64,
        cost_micros: row.get::<_, i64>(start + 5)? as u64,
    })
}

/// Read a SumStats from a GROUP BY row whose columns 1..=6 hold the six stats
/// (column 0 is the group key).
fn sum_stats_from_row(row: &Row<'_>) -> rusqlite::Result<SumStats> {
    sum_stats_from_row_at(row, 1)
}

fn record_from_row(row: &Row<'_>) -> rusqlite::Result<UsageRecord> {
    let provider_id: String = row.get("provider")?;
    let provider = Provider::from_id(&provider_id).ok_or_else(|| sqlite_error(&provider_id))?;
    let started_at: String = row.get("started_at")?;
    let started_at = parse_rfc3339(&started_at)?;
    Ok(UsageRecord {
        provider,
        project: row.get("project")?,
        session_id: row.get("session_id")?,
        usage: Usage {
            model: row.get("model")?,
            started_at,
            input_tokens: row.get::<_, i64>("input_tokens")? as u64,
            output_tokens: row.get::<_, i64>("output_tokens")? as u64,
            cache_read_tokens: row.get::<_, i64>("cache_read_tokens")? as u64,
            cache_write_tokens: row.get::<_, i64>("cache_write_tokens")? as u64,
            cost_micros: row.get::<_, i64>("cost_micros")? as u64,
        },
        raw_bytes: row.get::<_, i64>("raw_bytes")? as u64,
        fingerprint: row.get("fingerprint")?,
    })
}

fn sqlite_error(provider_id: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        format!("unknown provider id {provider_id:?}").into(),
    )
}

fn parse_rfc3339(s: &str) -> rusqlite::Result<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
        })
}
