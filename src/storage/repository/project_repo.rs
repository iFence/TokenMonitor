use std::path::{Path, PathBuf};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use crate::core::model::{Project, Provider};

pub struct ProjectRepo<'a> {
    conn: &'a Connection,
}

impl<'a> ProjectRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        ProjectRepo { conn }
    }

    /// Insert a project or update its path/providers if it already exists.
    pub fn upsert(&self, name: &str, path: &Path, providers: &[Provider]) -> Result<i64> {
        let providers_json = serde_json::to_string(providers)?;
        self.conn.execute(
            "INSERT INTO projects (name, path, providers) VALUES (?1, ?2, ?3)
             ON CONFLICT(name) DO UPDATE SET
               path = excluded.path,
               providers = excluded.providers",
            params![name, path.to_string_lossy(), providers_json],
        )?;
        Ok(self.by_name(name)?.expect("project was just upserted").id)
    }

    pub fn by_name(&self, name: &str) -> Result<Option<Project>> {
        self.conn
            .query_row(
                "SELECT id, name, path, providers FROM projects WHERE name = ?1",
                params![name],
                project_from_row,
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list(&self) -> Result<Vec<Project>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, name, path, providers FROM projects ORDER BY name")?;
        let rows = stmt.query_map([], project_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    let providers_json: String = row.get("providers")?;
    let providers = serde_json::from_str(&providers_json).unwrap_or_default();
    Ok(Project {
        id: row.get("id")?,
        name: row.get("name")?,
        path: PathBuf::from(row.get::<_, String>("path")?),
        providers,
    })
}
