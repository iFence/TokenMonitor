use std::path::PathBuf;

use anyhow::Result;
use rusqlite::Connection;

use crate::core::model::Provider;
use crate::providers::{ProviderError, ProviderSource};
use crate::storage::repository::{ProjectRepo, SettingsRepo, UsageRepo};

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

    let output = match source.scan() {
        Ok(o) => o,
        // Tool not installed on this machine — not an error.
        Err(ProviderError::DataDirNotFound(_)) => return Ok(summary),
        Err(e) => {
            summary.errors.push(e.to_string());
            return Ok(summary);
        }
    };
    summary.found_files = output.found_files;
    summary.records = output.records.len() as u64;

    let usage_repo = UsageRepo::new(conn);
    let batch = usage_repo.batch_insert_dedup(&output.records)?;
    summary.inserted = batch.inserted;
    summary.skipped_duplicates = batch.skipped_duplicates;

    let project_repo = ProjectRepo::new(conn);
    let mut names: Vec<&str> = output.records.iter().map(|r| r.project.as_str()).collect();
    names.sort_unstable();
    names.dedup();
    for name in names {
        project_repo.upsert(name, &PathBuf::new(), &[source.provider()])?;
        summary.projects.push(name.to_string());
    }

    if let Some(fp) = &fingerprint {
        settings.set(&fp_key, fp)?;
    }

    summary.errors = output.errors;
    Ok(summary)
}
