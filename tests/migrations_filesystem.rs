use anyhow::Context;
use anyhow::Result;
use rusqlite::OptionalExtension;
use std::fs;
use tempfile::TempDir;

#[tokio::test]
async fn test_filesystem_migrations_applied() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let migration_dir = temp_dir.path().join("migrations");
    let sqlite_dir = migration_dir.join("sqlite");
    fs::create_dir_all(&sqlite_dir)?;

    // Create a simple migration that will create a test table
    let migration_sql = r#"CREATE TABLE IF NOT EXISTS fs_migration_test (
    id INTEGER PRIMARY KEY,
    name TEXT
);"#;
    fs::write(
        sqlite_dir.join("V001__create_fs_test_table.sql"),
        migration_sql,
    )?;

    // Create a temp database file
    let tmp_db_dir = TempDir::new()?;
    let db_path = tmp_db_dir.path().join("fs_test.db");

    // Directly load and run filesystem migrations with refinery so the
    // test doesn't have to rely on environment variable side-effects.
    let migrations = refinery::load_sql_migrations(&sqlite_dir)
        .with_context(|| format!("loading migrations from {}", sqlite_dir.display()))?;

    let mut conn = rusqlite::Connection::open(&db_path)?;
    let runner_fs = refinery::Runner::new(&migrations)
        .set_abort_divergent(true)
        .set_abort_missing(true);
    runner_fs
        .run(&mut conn)
        .with_context(|| "running filesystem migrations in test")?;
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='fs_migration_test'",
    )?;
    let found: Option<String> = stmt.query_row([], |r| r.get(0)).optional()?;
    assert!(found.is_some());

    Ok(())
}
