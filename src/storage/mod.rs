//! SQLite persistence: connection + schema + repository layer.

pub mod repository;
pub mod sqlite;

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use rusqlite::{backup::Backup, Connection};

pub use sqlite::{init_schema, open, SCHEMA_SQL};

/// Default location of the TokenMonitor SQLite database.
pub fn default_db_path() -> Result<PathBuf> {
    let dir = crate::platform::app_data_dir().context("resolve app data dir")?;
    Ok(dir.join("tokenmonitor.db"))
}

/// One-time migration of the pre-rename `rToken` database.
///
/// Before the project was renamed `rtoken` → `tokenmonitor` the database lived
/// at `%APPDATA%\rToken\rtoken.db`; the current location is
/// `%APPDATA%\TokenMonitor\tokenmonitor.db`. The first time the app starts with
/// no database at the new path, copy the legacy file over (via the SQLite
/// backup API, so any WAL contents are checkpointed) so usage history survives
/// the rename.
///
/// Best-effort: failures are logged and the app starts with a fresh database.
pub fn migrate_legacy_db(new_path: &Path) {
    let legacy = match crate::platform::legacy_data_dir() {
        Ok(dir) => dir.join("rtoken.db"),
        Err(_) => return,
    };
    match migrate_db_file(&legacy, new_path) {
        Ok(()) => eprintln!(
            "TokenMonitor: migrated usage database from {} to {}",
            legacy.display(),
            new_path.display()
        ),
        Err(err) => eprintln!("TokenMonitor: legacy database migration skipped: {err:#}"),
    }
}

/// Copy a legacy SQLite file into `new_path` if it does not exist yet.
///
/// Backs up into a temp file and renames it into place only on success, so a
/// failed migration never leaves a half-written DB at the target path (which
/// would suppress future retries). No-op when the target already exists or the
/// legacy file is missing.
fn migrate_db_file(legacy: &Path, new_path: &Path) -> Result<()> {
    if new_path.exists() || !legacy.exists() {
        return Ok(());
    }
    let Some(parent) = new_path.parent() else {
        return Ok(());
    };
    std::fs::create_dir_all(parent).context("create db parent dir")?;
    let tmp = new_path.with_extension("db.migrating");
    let _ = std::fs::remove_file(&tmp);
    let result = (|| -> Result<()> {
        {
            let from = Connection::open(legacy)
                .with_context(|| format!("open legacy database {}", legacy.display()))?;
            let mut to =
                Connection::open(&tmp).with_context(|| format!("create {}", tmp.display()))?;
            let backup = Backup::new(&from, &mut to).context("init database backup")?;
            backup
                .run_to_completion(512, Duration::from_millis(50), None)
                .context("backup legacy database")?;
        }
        std::fs::rename(&tmp, new_path)
            .with_context(|| format!("move {} into place", tmp.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_legacy_database_file() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("rtoken.db");
        let new_path = dir.path().join("TokenMonitor").join("tokenmonitor.db");

        let conn = Connection::open(&legacy).unwrap();
        conn.execute_batch("CREATE TABLE t (x INTEGER); INSERT INTO t VALUES (42);")
            .unwrap();
        drop(conn);

        migrate_db_file(&legacy, &new_path).unwrap();
        assert!(new_path.exists());

        let read = Connection::open(&new_path).unwrap();
        let n: i64 = read.query_row("SELECT x FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn keeps_existing_target_unmodified() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("rtoken.db");
        std::fs::write(&legacy, b"legacy").unwrap();
        let new_path = dir.path().join("tokenmonitor.db");
        std::fs::write(&new_path, b"new").unwrap();

        migrate_db_file(&legacy, &new_path).unwrap();
        assert_eq!(std::fs::read_to_string(&new_path).unwrap(), "new");
    }

    #[test]
    fn noops_when_legacy_file_missing() {
        let dir = tempfile::tempdir().unwrap();
        let new_path = dir.path().join("tokenmonitor.db");

        migrate_db_file(&dir.path().join("missing.db"), &new_path).unwrap();
        assert!(!new_path.exists());
    }
}
