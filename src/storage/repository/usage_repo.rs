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
