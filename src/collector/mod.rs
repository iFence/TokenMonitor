//! Data-collection pipeline: scans, watches, and schedules provider data sync.

pub mod scanner;
pub mod scheduler;
pub mod watcher;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use async_channel::{unbounded, Receiver, Sender};
use rusqlite::Connection;

use crate::core::model::Provider;
use crate::providers::{build_sources, default_configs, ProviderSource};
use crate::storage::sqlite;

use scanner::{scan_all, scan_one, ScanSummary};

/// Events the collector emits to the app.
#[derive(Debug, Clone)]
pub enum CollectorEvent {
    ScanStarted { provider: Provider },
    ScanCompleted { summary: ScanSummary },
    ScanFailed { provider: Provider, error: String },
    Watch(watcher::WatchEvent),
}

/// Owns the SQLite connection and the provider sources; coordinates scans.
pub struct Collector {
    db_path: PathBuf,
    db: Arc<Mutex<Connection>>,
    tx: Sender<CollectorEvent>,
    rx: Receiver<CollectorEvent>,
    sources: Arc<Vec<Box<dyn ProviderSource>>>,
}

impl Collector {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = sqlite::open(db_path)?;
        let (tx, rx) = unbounded();
        let sources = Arc::new(build_sources(&default_configs()));
        Ok(Collector {
            db_path: db_path.to_path_buf(),
            db: Arc::new(Mutex::new(conn)),
            tx,
            rx,
            sources,
        })
    }

    /// Synchronous full scan (blocking); returns per-provider summaries.
    pub fn run_scan(&self) -> Result<Vec<ScanSummary>> {
        let conn = self.db.lock().expect("db lock poisoned");
        scan_all(&conn, &self.sources)
    }

    /// Kick off a scan on a background thread; results arrive as events.
    pub fn scan_async(&self) -> Result<()> {
        let db = self.db.clone();
        let sources = self.sources.clone();
        let tx = self.tx.clone();
        std::thread::Builder::new()
            .name("rtoken-scan".into())
            .spawn(move || {
                for source in sources.iter() {
                    let provider = source.provider();
                    let _ = tx.send_blocking(CollectorEvent::ScanStarted { provider });
                    let result = {
                        let conn = db.lock().expect("db lock poisoned");
                        scan_one(&conn, source.as_ref())
                    };
                    match result {
                        Ok(summary) => {
                            let _ = tx.send_blocking(CollectorEvent::ScanCompleted { summary });
                        }
                        Err(e) => {
                            let _ = tx.send_blocking(CollectorEvent::ScanFailed {
                                provider,
                                error: e.to_string(),
                            });
                        }
                    }
                }
            })?;
        Ok(())
    }

    pub fn events(&self) -> Receiver<CollectorEvent> {
        self.rx.clone()
    }

    pub fn db(&self) -> Arc<Mutex<Connection>> {
        self.db.clone()
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn sources(&self) -> Arc<Vec<Box<dyn ProviderSource>>> {
        self.sources.clone()
    }
}
