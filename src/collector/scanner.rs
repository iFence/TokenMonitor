use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Result;
use rusqlite::Connection;

use crate::core::model::Provider;
use crate::core::pricing::Pricer;
use crate::core::usage::UsageRecord;
use crate::providers::{FileStates, ProviderError, ProviderSource};
use crate::storage::repository::{BatchInsertStats, ProjectRepo, SettingsRepo, UsageRepo};

/// Records are buffered into batches of this size before the dedup insert, so
/// a full scan never holds every parsed record in memory at once.
const INSERT_BATCH: usize = 2000;

/// Outcome of scanning one provider.
#[derive(Debug, Clone, Default)]
pub struct ScanSummary {
    pub provider: Provider,
    pub found_files: u64,
    pub records: u64,
    pub inserted: u64,
    pub skipped_duplicates: u64,
    pub projects: Vec<String>,
    pub errors: Vec<String>,
    /// True when the source fingerprint matched, so nothing was read or written.
    pub unchanged: bool,
}

/// Scan every source, persisting normalized records.
pub fn scan_all(
    conn: &Connection,
    sources: &[Box<dyn ProviderSource>],
) -> Result<Vec<ScanSummary>> {
    let mut summaries = Vec::with_capacity(sources.len());
    for source in sources {
        summaries.push(scan_one(conn, source.as_ref())?);
    }
    Ok(summaries)
}

/// Scan one source: skip via fingerprint, then parse raw files, dedup-insert,
/// and upsert discovered projects.
pub fn scan_one(conn: &Connection, source: &dyn ProviderSource) -> Result<ScanSummary> {
    let mut summary = ScanSummary {
        provider: source.provider(),
        ..Default::default()
    };

    // Cheap change detector: when the source tree is unchanged since the last
    // successful scan, skip the read/parse/insert entirely (the DB already
    // holds the records). The fingerprint is persisted only after a successful
    // insert, so a crash mid-scan leaves the old value and forces a full
    // rescan next time; `INSERT OR IGNORE` keeps rescans idempotent.
    let settings = SettingsRepo::new(conn);
    let fp_key = format!("scan.fingerprint.{}", source.provider().id());
    let fingerprint = match source.scan_fingerprint() {
        Ok(fp) => Some(fp),
        // Tool not installed on this machine — not an error.
        Err(ProviderError::DataDirNotFound(_)) => return Ok(summary),
        Err(e) => {
            summary.errors.push(e.to_string());
            return Ok(summary);
        }
    };

    if let Some(fp) = &fingerprint {
        if settings.get(&fp_key)? == Some(fp.clone()) {
            summary.unchanged = true;
            return Ok(summary);
        }
    }

    // Per-file state from the last successful scan: lets the adapter skip
    // parsing files whose (mtime, size) hasn't changed since then. Corrupt or
    // absent state degrades to a full scan, never to missed data.
    let files_key = format!("scan.files.{}", source.provider().id());
    let known: FileStates = settings
        .get(&files_key)?
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();

    // Stream records out of the provider, dedup-inserting in bounded batches
    // instead of accumulating the whole scan before writing.
    let usage_repo = UsageRepo::new(conn);
    let mut batch: Vec<UsageRecord> = Vec::with_capacity(INSERT_BATCH);
    let mut insert_stats = BatchInsertStats::default();
    let mut projects: BTreeSet<String> = BTreeSet::new();
    let mut records: u64 = 0;
    let mut flush_err: Option<anyhow::Error> = None;

    let output = match source.scan_incremental(&mut |r| {
        records += 1;
        projects.insert(r.project.clone());
        // Pricing is the "later pipeline stage" the adapters defer to: resolve
        // the model against the embedded price table and stamp `cost_micros`
        // before the row is dedup-inserted.
        let mut r = r;
        r.usage.cost_micros = Pricer::global().cost_micros(
            r.provider,
            &r.usage.model,
            r.usage.input_tokens,
            r.usage.output_tokens,
            r.usage.cache_read_tokens,
            r.usage.cache_write_tokens,
        );
        batch.push(r);
        if batch.len() >= INSERT_BATCH {
            if flush_err.is_none() {
                match usage_repo.batch_insert_dedup(&batch) {
                    Ok(s) => {
                        insert_stats.inserted += s.inserted;
                        insert_stats.skipped_duplicates += s.skipped_duplicates;
                    }
                    Err(e) => flush_err = Some(e),
                }
            }
            batch.clear();
        }
    }, &known) {
        Ok(o) => o,
        // Tool not installed on this machine — not an error.
        Err(ProviderError::DataDirNotFound(_)) => return Ok(summary),
        Err(e) => {
            summary.errors.push(e.to_string());
            return Ok(summary);
        }
    };
    summary.found_files = output.found_files;
    summary.records = records;
    if let Some(e) = flush_err {
        return Err(e);
    }
    if !batch.is_empty() {
        let s = usage_repo.batch_insert_dedup(&batch)?;
        insert_stats.inserted += s.inserted;
        insert_stats.skipped_duplicates += s.skipped_duplicates;
    }
    summary.inserted = insert_stats.inserted;
    summary.skipped_duplicates = insert_stats.skipped_duplicates;

    let project_repo = ProjectRepo::new(conn);
    for name in projects {
        project_repo.upsert(&name, &PathBuf::new(), &[source.provider()])?;
        summary.projects.push(name);
    }

    // Persist the fresh per-file state before the fingerprint: if a crash lands
    // between the two, the stale fingerprint forces a rescan on the next start
    // (safe — it only redoes work, never misses data).
    if let Some(states) = &output.file_states {
        if let Ok(json) = serde_json::to_string(states) {
            settings.set(&files_key, &json)?;
        }
    }

    if let Some(fp) = &fingerprint {
        settings.set(&fp_key, fp)?;
    }

    summary.errors = output.errors;
    Ok(summary)
}
