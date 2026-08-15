//! Data-collection pipeline: scans, watches, and schedules provider data sync.

pub mod scanner;
pub mod scheduler;
pub mod watcher;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use anyhow::Result;
use async_channel::{unbounded, Receiver, Sender};
use rusqlite::Connection;

use crate::core::model::{Provider, ProviderSelection};
use crate::core::pricing::{Pricer, PRICING_VERSION, PRICING_VERSION_KEY};
use crate::providers::{build_sources, configs_for, ProviderSource};
use crate::storage::repository::{SettingsRepo, UsageRepo};
use crate::storage::sqlite;

use scanner::{scan_all, scan_one, ScanSummary};

/// Settings key holding the JSON-encoded [`ProviderSelection`].
const PROVIDER_SELECTION_KEY: &str = "providers.selection";

/// Settings keys for the auto-update preferences.
const CHECK_UPDATES_ON_STARTUP_KEY: &str = "update.check_on_startup";
const SKIPPED_UPDATE_VERSION_KEY: &str = "update.skipped_version";

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
    selection: RwLock<ProviderSelection>,
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
                    "rToken: backfilled cost for {updated} usage rows (pricing v{PRICING_VERSION})"
                );
            }
        }
        let (tx, rx) = unbounded();
        let selection = load_provider_selection(&conn)?;
        let sources = Arc::new(build_sources(&configs_for(&selection)));
        Ok(Collector {
            db_path: db_path.to_path_buf(),
            db: Arc::new(Mutex::new(conn)),
            tx,
            rx,
            selection: RwLock::new(selection),
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

    /// The current user app selection (order + enabled flags).
    pub fn selection(&self) -> ProviderSelection {
        self.selection
            .read()
            .expect("selection lock poisoned")
            .clone()
    }

    /// Replace the app selection: persist it and rebuild the enabled sources so
    /// the next scan reflects the new order and enabled set.
    pub fn set_selection(&self, selection: ProviderSelection) -> Result<()> {
        {
            let conn = self.db.lock().expect("db lock poisoned");
            save_provider_selection(&conn, &selection)?;
        }
        let sources = Arc::new(build_sources(&configs_for(&selection)));
        *self.sources.write().expect("sources lock poisoned") = sources;
        *self.selection.write().expect("selection lock poisoned") = selection;
        Ok(())
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

/// Load the persisted app selection, normalizing against the known provider set
/// so a fresh DB (or a partially written value) yields a valid selection.
fn load_provider_selection(conn: &Connection) -> Result<ProviderSelection> {
    let mut selection = match SettingsRepo::new(conn).get(PROVIDER_SELECTION_KEY)? {
        Some(json) => match serde_json::from_str::<ProviderSelection>(&json) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("rToken: ignoring corrupt provider selection ({e}); using defaults");
                ProviderSelection::default()
            }
        },
        None => ProviderSelection::default(),
    };
    selection.normalize();
    Ok(selection)
}

fn save_provider_selection(conn: &Connection, selection: &ProviderSelection) -> Result<()> {
    let json = serde_json::to_string(selection)?;
    SettingsRepo::new(conn).set(PROVIDER_SELECTION_KEY, &json)
}
