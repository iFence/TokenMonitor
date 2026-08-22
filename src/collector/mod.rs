//! Data-collection pipeline: scans, watches, and schedules provider data sync.

pub mod scanner;
pub mod scheduler;
pub mod watcher;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use async_channel::{unbounded, Receiver, Sender};
use rusqlite::Connection;

use crate::core::model::{Provider, ThemeColor};
use crate::core::pricing::{Pricer, PRICING_VERSION, PRICING_VERSION_KEY};
use crate::providers::{build_sources, default_configs, ProviderSource};
use crate::storage::repository::{SettingsRepo, UsageRepo};
use crate::storage::sqlite;

use scanner::{scan_all, scan_one, ScanSummary};

/// Settings keys for the auto-update preferences.
const CHECK_UPDATES_ON_STARTUP_KEY: &str = "update.check_on_startup";
const SKIPPED_UPDATE_VERSION_KEY: &str = "update.skipped_version";

/// Settings key holding the periodic rescan interval, in seconds.
const SCAN_INTERVAL_KEY: &str = "scan.interval_seconds";

/// Settings key holding the app accent theme color (a `ThemeColor` key).
const THEME_COLOR_KEY: &str = "theme.color";

/// Default periodic rescan interval (5 minutes).
pub const DEFAULT_SCAN_INTERVAL_SECS: u64 = 300;

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
    sources: RwLock<Arc<Vec<Box<dyn ProviderSource>>>>,
}

impl Collector {
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = sqlite::open(db_path)?;
        // One-time backfill: adapters leave `cost_micros` unset until the scan
        // pipeline stamps it, so rows written before this feature are all 0.
        // Re-cost everything whenever the embedded pricing data version bumps.
        {
            let settings = SettingsRepo::new(&conn);
            if settings.get(PRICING_VERSION_KEY)? != Some(PRICING_VERSION.to_string()) {
                let updated = UsageRepo::new(&conn).recompute_all_costs(Pricer::global())?;
                settings.set(PRICING_VERSION_KEY, PRICING_VERSION)?;
                eprintln!(
                    "TokenMonitor: backfilled cost for {updated} usage rows (pricing v{PRICING_VERSION})"
                );
            }
        }
        let (tx, rx) = unbounded();
        let sources = Arc::new(build_sources(&default_configs()));
        Ok(Collector {
            db_path: db_path.to_path_buf(),
            db: Arc::new(Mutex::new(conn)),
            tx,
            rx,
            sources: RwLock::new(sources),
        })
    }

    /// Synchronous full scan (blocking); returns per-provider summaries.
    pub fn run_scan(&self) -> Result<Vec<ScanSummary>> {
        let conn = self.db.lock().expect("db lock poisoned");
        let sources = self.sources.read().expect("sources lock poisoned").clone();
        scan_all(&conn, &sources)
    }

    /// Kick off a scan on a background thread; results arrive as events.
    pub fn scan_async(&self) -> Result<()> {
        let db = self.db.clone();
        let sources = self.sources.read().expect("sources lock poisoned").clone();
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
        self.sources.read().expect("sources lock poisoned").clone()
    }

    /// Whether to check for updates on startup (defaults to `true`).
    pub fn check_updates_on_startup(&self) -> bool {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn)
            .get(CHECK_UPDATES_ON_STARTUP_KEY)
            .ok()
            .flatten()
            .map(|value| value == "true")
            .unwrap_or(true)
    }

    pub fn set_check_updates_on_startup(&self, enabled: bool) -> Result<()> {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn).set(
            CHECK_UPDATES_ON_STARTUP_KEY,
            if enabled { "true" } else { "false" },
        )
    }

    /// The periodic rescan interval in seconds (defaults to 5 minutes).
    pub fn scan_interval_seconds(&self) -> u64 {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn)
            .get(SCAN_INTERVAL_KEY)
            .ok()
            .flatten()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_SCAN_INTERVAL_SECS)
    }

    pub fn set_scan_interval_seconds(&self, secs: u64) -> Result<()> {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn).set(SCAN_INTERVAL_KEY, &secs.to_string())
    }

    /// The app accent theme color (defaults to ocean blue).
    pub fn theme_color(&self) -> ThemeColor {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn)
            .get(THEME_COLOR_KEY)
            .ok()
            .flatten()
            .map(|value| ThemeColor::from_key(&value))
            .unwrap_or_default()
    }

    pub fn set_theme_color(&self, color: ThemeColor) -> Result<()> {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn).set(THEME_COLOR_KEY, color.key())
    }

    /// The update version the user last chose to skip, if any.
    pub fn skipped_update_version(&self) -> Option<String> {
        let conn = self.db.lock().expect("db lock poisoned");
        SettingsRepo::new(&conn)
            .get(SKIPPED_UPDATE_VERSION_KEY)
            .ok()
            .flatten()
    }

    pub fn set_skipped_update_version(&self, version: Option<&str>) -> Result<()> {
        let conn = self.db.lock().expect("db lock poisoned");
        let repo = SettingsRepo::new(&conn);
        match version {
            Some(version) => repo.set(SKIPPED_UPDATE_VERSION_KEY, version),
            None => repo.remove(SKIPPED_UPDATE_VERSION_KEY),
        }
    }
}
