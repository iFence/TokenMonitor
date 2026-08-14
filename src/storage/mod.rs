//! SQLite persistence: connection + schema + repository layer.

pub mod repository;
pub mod sqlite;

use anyhow::{Context, Result};
use std::path::PathBuf;

pub use sqlite::{init_schema, open, SCHEMA_SQL};

/// Default location of the rToken SQLite database.
pub fn default_db_path() -> Result<PathBuf> {
    let dir = crate::platform::app_data_dir().context("resolve app data dir")?;
    Ok(dir.join("rtoken.db"))
}
