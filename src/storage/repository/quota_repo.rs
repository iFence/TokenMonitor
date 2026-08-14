use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::core::model::{Period, Provider, Quota};

pub struct QuotaRepo<'a> {
    conn: &'a Connection,
}

impl<'a> QuotaRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        QuotaRepo { conn }
    }

    pub fn get(&self, provider: Provider, period: Period) -> Result<Option<Quota>> {
        self.conn
            .query_row(
                "SELECT provider, period, limit_tokens, used_tokens,
                        limit_cost_micros, used_cost_micros, updated_at
                 FROM quotas WHERE provider = ?1 AND period = ?2",
                params![provider.id(), period.id()],
                quota_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn upsert(&self, quota: &Quota) -> Result<()> {
        self.conn.execute(
            "INSERT INTO quotas
             (provider, period, limit_tokens, used_tokens,
              limit_cost_micros, used_cost_micros, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(provider, period) DO UPDATE SET
               limit_tokens = excluded.limit_tokens,
               used_tokens = excluded.used_tokens,
               limit_cost_micros = excluded.limit_cost_micros,
               used_cost_micros = excluded.used_cost_micros,
               updated_at = excluded.updated_at",
            params![
                quota.provider.id(),
                quota.period.id(),
                quota.limit_tokens as i64,
                quota.used_tokens as i64,
                quota.limit_cost_micros as i64,
                quota.used_cost_micros as i64,
                quota.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<Quota>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM quotas ORDER BY provider, period")?;
        let rows = stmt.query_map([], quota_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn quota_from_row(row: &Row<'_>) -> rusqlite::Result<Quota> {
    let provider_id: String = row.get("provider")?;
    let period_id: String = row.get("period")?;
    let provider = Provider::from_id(&provider_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown provider id {provider_id:?}").into(),
        )
    })?;
    let period = Period::from_id(&period_id).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Text,
            format!("unknown period id {period_id:?}").into(),
        )
    })?;
    let updated_at: String = row.get("updated_at")?;
    let updated_at = chrono::DateTime::parse_from_rfc3339(&updated_at)
        .map(|dt| dt.with_timezone(&chrono::Utc))
        .map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
        })?;
    Ok(Quota {
        provider,
        period,
        limit_tokens: row.get::<_, i64>("limit_tokens")? as u64,
        used_tokens: row.get::<_, i64>("used_tokens")? as u64,
        limit_cost_micros: row.get::<_, i64>("limit_cost_micros")? as u64,
        used_cost_micros: row.get::<_, i64>("used_cost_micros")? as u64,
        updated_at,
    })
}
