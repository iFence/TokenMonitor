use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Settings key holding the desktop window size as `WIDTHxHEIGHT` (logical px).
const WINDOW_SIZE_KEY: &str = "window.size";

pub struct SettingsRepo<'a> {
    conn: &'a Connection,
}

impl<'a> SettingsRepo<'a> {
    pub fn new(conn: &'a Connection) -> Self {
        SettingsRepo { conn }
    }

    pub fn get(&self, key: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// The window size the desktop frontend last saved, if any. Malformed or
    /// degenerate values are ignored rather than failing the startup path that
    /// reads them, so a hand-edited settings row cannot stop the app from
    /// opening.
    pub fn window_size(&self) -> Result<Option<(f32, f32)>> {
        let Some(value) = self.get(WINDOW_SIZE_KEY)? else {
            return Ok(None);
        };
        Ok(parse_window_size(&value))
    }

    /// Persist the desktop window size, rounded to whole logical pixels.
    pub fn set_window_size(&self, width: f32, height: f32) -> Result<()> {
        self.set(
            WINDOW_SIZE_KEY,
            &format!("{}x{}", width.round(), height.round()),
        )
    }

    pub fn remove(&self, key: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM settings WHERE key = ?1", params![key])?;
        Ok(())
    }
}

/// Parse `WIDTHxHEIGHT`; anything non-positive or unparsable yields `None`.
fn parse_window_size(value: &str) -> Option<(f32, f32)> {
    let (width, height) = value.split_once('x')?;
    let width: f32 = width.trim().parse().ok()?;
    let height: f32 = height.trim().parse().ok()?;
    (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
        .then_some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sqlite;

    #[test]
    fn window_size_round_trips_and_ignores_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let conn = sqlite::open(&dir.path().join("test.db")).unwrap();
        let repo = SettingsRepo::new(&conn);

        assert_eq!(repo.window_size().unwrap(), None);

        repo.set_window_size(1234.6, 800.4).unwrap();
        assert_eq!(repo.window_size().unwrap(), Some((1235.0, 800.0)));

        for garbage in ["", "1100", "1100x", "x800", "0x800", "1100x0", "a x b"] {
            repo.set(WINDOW_SIZE_KEY, garbage).unwrap();
            assert_eq!(repo.window_size().unwrap(), None, "input: {garbage:?}");
        }
    }
}
