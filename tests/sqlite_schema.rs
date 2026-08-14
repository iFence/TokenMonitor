use rusqlite::Connection;

use rtoken::storage::sqlite::{init_schema, SCHEMA_SQL};

#[test]
fn schema_creates_expected_tables() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    let tables: Vec<String> = conn
        .prepare(
            "SELECT name FROM sqlite_master
             WHERE type = 'table' AND name NOT LIKE 'sqlite_%'
             ORDER BY name",
        )
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(
        tables,
        vec!["projects", "quotas", "settings", "usage_records"]
    );
}

#[test]
fn schema_is_idempotent() {
    let conn = Connection::open_in_memory().unwrap();
    init_schema(&conn).unwrap();
    // A second run must not fail on existing tables/indexes.
    init_schema(&conn).unwrap();
}

#[test]
fn ddl_sql_runs_standalone() {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(SCHEMA_SQL).unwrap();
}
