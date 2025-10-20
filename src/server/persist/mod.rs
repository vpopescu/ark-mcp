//! Persistent storage implementation for Ark MCP server.
//!
//! This module provides database functionality for storing and managing:
//! - User sessions with automatic expiry
//! - Plugin metadata and ownership information
//!
//! The database uses SQLite with secure file permissions and optimized settings
//! for server workloads. All operations are async-compatible using blocking
//! task spawning.

use anyhow::{Context, Result};
use refinery::Runner;
use refinery::embed_migrations;
use rusqlite::{Connection, params};

// Embed compile-time migrations located under `migrations/sqlite/`.
// This macro expands to an `embedded_migrations` module with a `runner()` helper.
embed_migrations!("migrations/sqlite");
#[cfg(unix)]
use fs2::FileExt;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use std::time::Instant;
use std::{
    env, fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
mod windows_lock {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::time::Duration;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
    use windows::core::PCWSTR;

    pub struct NamedMutexGuard(HANDLE);

    impl Drop for NamedMutexGuard {
        fn drop(&mut self) {
            unsafe {
                let _ = ReleaseMutex(self.0);
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn mutex_name_from_lock_path(lock_path: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(lock_path.to_string_lossy().as_bytes());
        let digest = hex::encode(hasher.finalize());
        format!("Global\\ark_migrate_{}", digest)
    }

    pub fn acquire(lock_path: &Path, timeout: Duration) -> anyhow::Result<NamedMutexGuard> {
        // Create a well-known mutex name derived from the lock path so all
        // processes targeting the same DB will use the same kernel object.
        let name = mutex_name_from_lock_path(lock_path);
        let wide: Vec<u16> = OsStr::new(&name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let pcw = PCWSTR(wide.as_ptr());

        unsafe {
            // CreateMutexW returns a Result<HANDLE> - unwrap with `?` to
            // propagate errors as anyhow errors.
            let handle = CreateMutexW(None, false, pcw)?;
            let ms: u32 = match timeout.as_millis().try_into() {
                Ok(v) => v,
                Err(_) => u32::MAX,
            };
            let wait = WaitForSingleObject(handle, ms);
            // Compare the numeric result values (WAIT_OBJECT_0 == 0,
            // WAIT_TIMEOUT == 0x102) to determine success vs timeout.
            let wait_val: u32 = wait.0 as u32;
            if wait_val == 0 {
                return Ok(NamedMutexGuard(handle));
            }
            // On timeout or other failure, close the handle and return error.
            let _ = CloseHandle(handle);
            if wait_val == 0x102 {
                return Err(anyhow::anyhow!("timeout waiting for named mutex"));
            }
            Err(anyhow::anyhow!(
                "waiting for named mutex failed: {}",
                wait_val
            ))
        }
    }
}

/// Opens a lock file with retry logic for Unix systems.
///
/// On Unix, file locking can fail if another process is holding the file open.
/// This function retries opening the file with a timeout to handle concurrent startup.
#[cfg(unix)]
fn open_lock_file_with_retry(lock_path: &Path, timeout: Duration) -> anyhow::Result<std::fs::File> {
    use std::thread::sleep;

    let start = Instant::now();
    loop {
        // First preference: open for read+write and create if needed.
        match OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(lock_path)
        {
            Ok(f) => {
                // Try to acquire exclusive lock
                match f.try_lock_exclusive() {
                    Ok(()) => return Ok(f),
                    Err(_) => {
                        if start.elapsed() > timeout {
                            return Err(anyhow::anyhow!(
                                "timeout acquiring migration lock {}: {}",
                                lock_path.display(),
                                timeout.as_secs()
                            ));
                        }
                        sleep(Duration::from_millis(100));
                    }
                }
            }
            Err(e) => {
                if start.elapsed() > timeout {
                    return Err(anyhow::anyhow!(
                        "timeout opening migration lock file {}: {}",
                        lock_path.display(),
                        e
                    ));
                }
                tracing::debug!(
                    "open migration lock {} failed (will retry): {}",
                    lock_path.display(),
                    e
                );
                sleep(Duration::from_millis(100));
            }
        }
    }
}

#[cfg(windows)]
type LockGuard = windows_lock::NamedMutexGuard;
#[cfg(unix)]
type LockGuard = (std::fs::File, PathBuf);

/// Unified cross-platform migration lock guard.
///
/// Abstracts away the differences between Windows named mutexes and Unix file locks.
#[allow(dead_code)]
struct MigrationLockGuard(LockGuard);

/// Opens a SQLite connection with optimized settings for server workloads.
///
/// This is a standalone version of Database::open for use in helper functions.
fn open_db_connection(db_path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;
    // Reasonable defaults for server workload
    conn.pragma_update(None, "journal_mode", "WAL").ok();
    conn.pragma_update(None, "synchronous", "NORMAL").ok();
    conn.pragma_update(None, "busy_timeout", 5000i64).ok(); // 5s
    Ok(conn)
}

impl MigrationLockGuard {
    /// Acquires a migration lock with the specified timeout.
    ///
    /// On Windows, uses a named mutex derived from the lock path.
    /// On Unix, uses file-based advisory locking with retry logic.
    fn new(lock_path: &Path, timeout: Duration) -> anyhow::Result<Self> {
        #[cfg(windows)]
        {
            let guard = windows_lock::acquire(lock_path, timeout)
                .with_context(|| format!("acquiring named mutex for {}", lock_path.display()))?;
            tracing::debug!("Acquired Windows named-mutex for {}", lock_path.display());
            Ok(MigrationLockGuard(guard))
        }
        #[cfg(unix)]
        {
            let file = open_lock_file_with_retry(lock_path, timeout)?;
            tracing::debug!("Acquired Unix file lock for {}", lock_path.display());
            Ok(MigrationLockGuard((file, lock_path.to_path_buf())))
        }
    }
}

impl Drop for MigrationLockGuard {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            // NamedMutexGuard handles cleanup automatically
        }
        #[cfg(unix)]
        {
            let (file, path) = &self.0;
            let _ = file.unlock();
            let _ = fs::remove_file(path);
        }
    }
}

/// Applies database migrations, preferring filesystem migrations if available.
///
/// If ARK_MIGRATIONS_DIR is set, loads and applies migrations from that directory.
/// Otherwise, applies embedded migrations bundled with the binary.
fn apply_migrations(db_path: &Path, migrations_dir: Option<&str>) -> anyhow::Result<()> {
    if let Some(dir) = migrations_dir {
        let dir_path = PathBuf::from(dir);
        if !dir_path.exists() {
            tracing::warn!(
                "ARK_MIGRATIONS_DIR {} does not exist; skipping filesystem migrations",
                dir_path.display()
            );
            return Ok(());
        }

        tracing::info!("Applying filesystem migrations from {}", dir);
        let migrations = refinery::load_sql_migrations(&dir_path)
            .with_context(|| format!("loading migrations from {}", dir_path.display()))?;

        let mut conn = open_db_connection(db_path)?;
        tracing::info!(
            "Applying {} filesystem migrations via refinery",
            migrations.len()
        );
        let runner = Runner::new(&migrations)
            .set_abort_divergent(true)
            .set_abort_missing(true);
        runner
            .run(&mut conn)
            .with_context(|| "applying filesystem migrations via refinery")?;
        tracing::debug!("Filesystem migrations applied successfully (refinery)");
    } else {
        tracing::info!("Applying embedded refinery migrations");
        let mut conn = open_db_connection(db_path)?;
        migrations::runner()
            .run(&mut conn)
            .with_context(|| "applying embedded migrations")?;
        tracing::debug!("Embedded migrations applied successfully");
    }
    Ok(())
}

use tokio::task;

use crate::utility::{set_secure_dir_permissions, set_secure_file_permissions};

pub mod models;
pub use models::{PluginRecord, SessionRecord};

/// SQLite database handle for persistent storage.
///
/// Provides async-compatible database operations for sessions and plugins.
#[derive(Clone, Debug)]
pub struct Database {
    /// Path to the SQLite database file.
    db_path: PathBuf,
}

// Keep the impl visible for tests; silence analyzer-only dead-code warnings.
#[allow(dead_code)]
impl Database {
    ///
    /// Returns an error if:
    /// - Directory creation fails
    /// - Database migrations fail
    /// - Permission setting fails
    pub fn new() -> Result<Self> {
        let path = resolve_db_path()?;
        tracing::debug!("Initializing database at path: {}", path.display());
        ensure_parent_dir(&path)?;
        let db = Self {
            db_path: path.clone(),
        };
        db.run_bootstrap_migrations()?;

        // Set secure permissions on the database file after creation
        if path.exists() {
            set_secure_file_permissions(&path).with_context(|| {
                format!(
                    "setting secure permissions on database file {}",
                    path.display()
                )
            })?;
        }

        tracing::debug!("Database initialized successfully at: {}", path.display());
        Ok(db)
    }

    /// Creates a new Database handle with an explicit database file path.
    ///
    /// This method performs the same initialization as [`Database::new`] but
    /// uses the provided path instead of the default platform path.
    ///
    /// # Arguments
    ///
    /// * `path` - The filesystem path where the database file should be created
    ///
    /// # Returns
    ///
    /// A configured `Database` instance using the specified path.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Parent directory creation fails
    /// - Database migrations fail
    /// - Permission setting fails
    // Public helper used by integration tests (tests/*). The IDE/static analyzer
    // can't always see usages from external integration test crates and will
    // incorrectly flag these as dead code. Keep the function public for tests
    // and silence the analyzer-only warning.
    #[allow(dead_code)]
    pub fn with_path<P: AsRef<Path>>(path: P) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        tracing::debug!("Initializing database at explicit path: {}", path.display());
        ensure_parent_dir(&path)?;
        let db = Self {
            db_path: path.clone(),
        };
        db.run_bootstrap_migrations()?;

        // Set secure permissions on the database file after creation
        if path.exists() {
            set_secure_file_permissions(&path).with_context(|| {
                format!(
                    "setting secure permissions on database file {}",
                    path.display()
                )
            })?;
        }

        tracing::debug!("Database initialized successfully at: {}", path.display());
        Ok(db)
    }

    /// Opens a SQLite connection with optimized settings for server workloads.
    ///
    /// Configures the connection with:
    /// - WAL (Write-Ahead Logging) mode for better concurrency
    /// - NORMAL synchronous mode for balanced performance/durability
    /// - 5 second busy timeout for handling concurrent access
    ///
    /// # Returns
    ///
    /// A configured SQLite connection.
    ///
    /// # Errors
    ///
    /// Returns an error if the database file cannot be opened.
    fn open(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)
            .with_context(|| format!("opening sqlite db at {}", self.db_path.display()))?;
        // Reasonable defaults for server workload
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        conn.pragma_update(None, "busy_timeout", 5000i64).ok(); // 5s
        Ok(conn)
    }

    /// Runs bootstrap migrations to create the initial database schema.
    ///
    /// Creates the following tables if they don't exist:
    ///
    /// **sessions table:**
    /// - `session_id` (TEXT PRIMARY KEY) - Unique session identifier
    /// - `principal_json` (TEXT) - Serialized user principal data
    /// - `expiry_utc` (TEXT) - ISO 8601 UTC expiration timestamp
    /// - Index on `expiry_utc` for efficient cleanup
    ///
    /// **plugins table:**
    /// - `owner` (TEXT) - Plugin owner identifier
    /// - `plugin_id` (TEXT) - Plugin identifier (typically the plugin name)
    /// - `plugin_name` (TEXT) - Friendly plugin name (kept for clarity / future use)
    /// - `plugin_path` (TEXT) - Original plugin path/URL used to load the plugin
    /// - `plugin_data` (BLOB) - Raw plugin payload (WASM bytes when available)
    /// - `metadata` (TEXT) - Serialized plugin metadata (JSON)
    /// - `date_added_utc` (TEXT) - ISO 8601 UTC creation timestamp
    /// - Composite primary key on `(owner, plugin_id)`
    /// - Indexes on `owner` and `date_added_utc`
    ///
    /// # Returns
    ///
    /// `Ok(())` if all migrations succeed.
    ///
    /// # Errors
    ///
    /// Returns an error if any SQL statement fails to execute.
    fn run_bootstrap_migrations(&self) -> Result<()> {
        // Embedded migrations are declared at module scope via
        // `refinery::embed_migrations!("migrations/sqlite")`.

        let auto = std::env::var("ARK_AUTO_APPLY_MIGRATIONS").unwrap_or_else(|_| "true".into());
        if auto.to_lowercase() == "false" {
            tracing::info!(
                "Automatic migration application disabled via ARK_AUTO_APPLY_MIGRATIONS"
            );
            return Ok(());
        }

        let _guard = MigrationLockGuard::new(
            &self.db_path.with_extension("migrate.lock"),
            Duration::from_secs(30),
        )?;
        let migrations_dir = std::env::var("ARK_MIGRATIONS_DIR").ok();
        apply_migrations(&self.db_path, migrations_dir.as_deref())?;
        Ok(())
    }

    // ---------------- Async Sessions ----------------

    // ...removed legacy convenience `save_session_async` in favor of the
    // model-based `save_session_record_async`.

    /// Model-based session writer: persists a full `SessionRecord`.
    ///
    /// This is the canonical session write path — callers should construct a
    /// `models::SessionRecord` (using the helper constructors) and call this
    /// function to persist it. Backwards-compatible helpers like
    /// `save_session_async` delegate to this implementation.
    pub async fn save_session_record_async(&self, record: models::SessionRecord) -> Result<()> {
        tracing::trace!(
            "Saving session record: session_id={}, expiry_epoch={}",
            record.session_id,
            record.expiry_epoch
        );
        let db_path = self.db_path.clone();
        let sid = record.session_id.clone();
        let principal_clone = record.principal.clone();
        let expiry_epoch = record.expiry_epoch;
        let expiry_utc_str = record.expiry_utc.to_rfc3339();
        let is_admin_flag: i64 = if record.is_admin { 1 } else { 0 };

        let result = task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;
            conn.pragma_update(None, "journal_mode", "WAL").ok();
            conn.pragma_update(None, "synchronous", "NORMAL").ok();

            let principal_json = serde_json::to_string(&principal_clone)?;
            conn.execute(
                r#"
                INSERT INTO sessions(session_id, principal_json, expiry_utc, expiry_epoch, is_admin)
                VALUES(?1, ?2, ?3, ?4, ?5)
                ON CONFLICT(session_id) 
                DO UPDATE SET 
                    principal_json = excluded.principal_json, 
                    expiry_utc = excluded.expiry_utc,
                    expiry_epoch = excluded.expiry_epoch,
                    is_admin = excluded.is_admin
                "#,
                params![
                    sid,
                    principal_json,
                    expiry_utc_str,
                    expiry_epoch,
                    is_admin_flag
                ],
            )?;
            Ok(())
        })
        .await?;
        tracing::trace!(
            "Session record saved successfully: session_id={}",
            record.session_id
        );
        result
    }

    /// Retrieves a session record from the database by session ID.
    ///
    /// Looks up the session and returns a typed `SessionRecord` model which
    /// includes the deserialized `Principal`, an RFC3339 `expiry_utc` and
    /// convenience `expiry_epoch`. Callers should check `expiry_utc` to
    /// determine whether the session has expired.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session identifier to look up
    ///
    /// # Returns
    ///
    /// - `Ok(Some((principal, expiry)))` if session exists
    /// - `Ok(None)` if session not found
    /// - `Err(...)` if database operation fails
    pub async fn get_session_record_async(
        &self,
        session_id: String,
    ) -> Result<Option<models::SessionRecord>> {
        tracing::trace!("Getting session: session_id={}", session_id);
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<Option<models::SessionRecord>> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            let mut stmt = conn.prepare(
                r#"SELECT session_id, principal_json, expiry_epoch, is_admin FROM sessions WHERE session_id = ?1"#,
            )?;

            tracing::trace!("Executing SQL: SELECT session_id, principal_json, expiry_epoch, is_admin FROM sessions WHERE session_id = {}", session_id);
            let rec: Option<(String, String, i64, Option<i64>)> = match stmt.query_row(params![session_id], |row| {
                Ok::<_, rusqlite::Error>( (
                    row.get(0)?, // session_id
                    row.get(1)?, // principal_json
                    row.get(2)?, // expiry_epoch
                    row.get::<_, Option<i64>>(3)?, // is_admin
                ))
            }) {
                Ok(v) => Some(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(e.into()),
            };

            if let Some((sid, principal_json, expiry_epoch, is_admin_opt)) = rec {
                match models::SessionRecord::from_db_row(sid.clone(), principal_json, expiry_epoch, is_admin_opt) {
                    Ok(session_record) => {
                        tracing::trace!("Session found: session_id={}, expiry_epoch={}", sid, session_record.expiry_epoch);
                        Ok(Some(session_record))
                    }
                    Err(e) => {
                        tracing::warn!(error=%e, "Skipping malformed session row: {}", sid);
                        Err(e)
                    }
                }
            } else {
                tracing::trace!("Session not found: session_id={}", session_id);
                Ok(None)
            }
        })
        .await?
    }

    // Transitional wrapper `get_session_async` removed — callers should use
    // the canonical `get_session_record_async` model-based API.

    /// Deletes a session from the database.
    ///
    /// # Arguments
    ///
    /// * `session_id` - The session identifier to delete
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if session was found and deleted
    /// - `Ok(false)` if session was not found
    /// - `Err(...)` if database operation fails
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection or delete operation fails.
    pub async fn delete_session_async(&self, session_id: String) -> Result<bool> {
        tracing::trace!("Deleting session: session_id={}", session_id);
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<bool> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            tracing::trace!(
                "Executing SQL: DELETE FROM sessions WHERE session_id = {}",
                session_id
            );
            let n = conn.execute(
                r#"DELETE FROM sessions WHERE session_id = ?1"#,
                params![session_id],
            )?;
            let deleted = n > 0;
            tracing::trace!(
                "Session deletion result: session_id={}, deleted={}",
                session_id,
                deleted
            );
            Ok(deleted)
        })
        .await?
    }

    /// Removes all expired sessions from the database.
    ///
    /// Deletes sessions where the expiry timestamp is less than or equal to
    /// the current UTC time. Should be called periodically to prevent
    /// database growth.
    ///
    /// # Returns
    ///
    /// - `Ok(count)` - Number of sessions deleted
    /// - `Err(...)` if database operation fails
    pub async fn cleanup_expired_sessions_async(&self) -> Result<usize> {
        tracing::trace!("Cleaning up expired sessions");
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<usize> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            let now_epoch = chrono::Utc::now().timestamp();
            tracing::trace!(
                "Executing SQL: DELETE FROM sessions WHERE expiry_epoch <= {}",
                now_epoch
            );
            let n = conn.execute(
                r#"DELETE FROM sessions WHERE expiry_epoch <= ?1"#,
                params![now_epoch],
            )?;
            tracing::trace!("Cleaned up {} expired sessions", n);
            Ok(n)
        })
        .await?
    }

    // ---------------- Async Plugins ----------------

    /// Inserts or updates plugin metadata in the database.
    ///
    /// Uses SQL UPSERT to either insert a new plugin record or update
    /// the existing one if the (owner, plugin_id) combination already exists.
    ///
    /// # Arguments
    ///
    /// * `owner` - Plugin owner identifier (typically from `Principal::global_id()`)
    /// * `plugin_id` - Unique plugin identifier
    /// * `metadata` - JSON metadata for the plugin
    ///
    /// # Returns
    ///
    /// `Ok(())` if the operation succeeds.
    ///
    /// Note: this convenience function now constructs a `PluginRecord` and
    /// delegates to the model-based writer `save_plugin_record_async`.
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Database connection fails
    /// - JSON serialization fails
    /// - SQL operation fails
    pub async fn save_plugin_record_async(&self, record: PluginRecord) -> Result<()> {
        tracing::trace!(
            "Saving plugin record: owner={}, plugin_id={}",
            record.owner,
            record.plugin_id
        );
        let db_path = self.db_path.clone();
        let owner = record.owner.clone();
        let plugin_id = record.plugin_id.clone();
        let plugin_name = record
            .plugin_name
            .clone()
            .or_else(|| Some(record.plugin_id.clone()))
            .unwrap();
        let plugin_path = record.plugin_path.clone();
        let plugin_data = record.plugin_data.clone();
        let metadata_json = serde_json::to_string(&record.metadata)?;
        let date_added_utc = record.date_added_utc.to_rfc3339();

        let result = task::spawn_blocking(move || -> Result<()> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;
            conn.pragma_update(None, "journal_mode", "WAL").ok();
            conn.pragma_update(None, "synchronous", "NORMAL").ok();

            conn.execute(
                r#"
                INSERT INTO plugins(owner, plugin_id, plugin_name, plugin_path, plugin_data, metadata, date_added_utc)
                VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT(owner, plugin_id) 
                DO UPDATE SET 
                    plugin_name = COALESCE(excluded.plugin_name, plugins.plugin_name),
                    plugin_path = COALESCE(excluded.plugin_path, plugins.plugin_path),
                    plugin_data = COALESCE(excluded.plugin_data, plugins.plugin_data),
                    metadata = excluded.metadata,
                    date_added_utc = excluded.date_added_utc
                "#,
                params![
                    owner,
                    plugin_id,
                    plugin_name,
                    plugin_path,
                    plugin_data,
                    metadata_json,
                    date_added_utc
                ],
            )?;
            Ok(())
        })
        .await?;
        tracing::trace!(
            "Plugin record saved successfully: owner={}, plugin_id={}",
            record.owner,
            record.plugin_id
        );
        result
    }

    /// Retrieves a specific plugin record by owner and plugin ID.
    ///
    /// # Arguments
    ///
    /// * `owner` - Plugin owner identifier
    /// * `plugin_id` - Plugin identifier
    ///
    /// # Returns
    ///
    /// - `Ok(Some(record))` if plugin exists
    /// - `Ok(None)` if plugin not found
    /// - `Err(...)` if database operation fails
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Database connection fails
    /// - JSON deserialization fails
    /// - Date parsing fails
    // Used by integration tests; keep public for test access and suppress
    // editor-only dead-code warnings.
    #[allow(dead_code)]
    pub async fn get_plugin_async(
        &self,
        owner: String,
        plugin_id: String,
    ) -> Result<Option<PluginRecord>> {
        tracing::trace!("Getting plugin: owner={}, plugin_id={}", owner, plugin_id);
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<Option<PluginRecord>> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            let mut stmt = conn.prepare(
                r#"SELECT owner, plugin_id, plugin_name, plugin_path, metadata, date_added_utc, plugin_data FROM plugins WHERE owner = ?1 AND plugin_id = ?2"#,
            )?;

            tracing::trace!("Executing SQL: SELECT FROM plugins WHERE owner = {} AND plugin_id = {}", owner, plugin_id);
            type PluginRow = (
                String,
                String,
                Option<String>,
                Option<String>,
                String,
                String,
                Option<Vec<u8>>,
            );
            let rec: Option<PluginRow> = match stmt.query_row(params![owner, plugin_id], |row| {
                Ok::<_, rusqlite::Error>((
                    row.get(0)?, // owner
                    row.get(1)?, // plugin_id
                    row.get::<_, Option<String>>(2)?, // plugin_name
                    row.get::<_, Option<String>>(3)?, // plugin_path
                    row.get(4)?, // metadata
                    row.get(5)?, // date_added_utc
                    row.get::<_, Option<Vec<u8>>>(6)?, // plugin_data
                ))
            }) {
                Ok(v) => Some(v),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(e.into()),
            };
            if let Some((owner, plugin_id, plugin_name, plugin_path, metadata_json, date_added_utc_str, plugin_data)) = rec {
                match PluginRecord::from_db_row(
                    owner,
                    plugin_id,
                    plugin_name,
                    plugin_path,
                    metadata_json,
                    date_added_utc_str,
                    plugin_data,
                ) {
                    Ok(record) => Ok(Some(record)),
                    Err(e) => {
                        tracing::warn!(error=%e, "Skipping malformed plugin row when fetching single plugin");
                        Err(e)
                    }
                }
            } else {
                tracing::trace!("Plugin not found");
                Ok(None)
            }
        })
        .await?
    }

    /// Deletes a plugin record from the database.
    ///
    /// # Arguments
    ///
    /// * `owner` - Plugin owner identifier
    /// * `plugin_id` - Plugin identifier
    ///
    /// # Returns
    ///
    /// - `Ok(true)` if plugin was found and deleted
    /// - `Ok(false)` if plugin was not found
    /// - `Err(...)` if database operation fails
    ///
    /// # Errors
    ///
    /// Returns an error if the database connection or delete operation fails.
    pub async fn delete_plugin_async(&self, owner: String, plugin_id: String) -> Result<bool> {
        tracing::trace!("Deleting plugin: owner={}, plugin_id={}", owner, plugin_id);
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<bool> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            tracing::trace!(
                "Executing SQL: DELETE FROM plugins WHERE owner = {} AND plugin_id = {}",
                owner,
                plugin_id
            );
            let n = conn.execute(
                r#"DELETE FROM plugins WHERE owner = ?1 AND plugin_id = ?2"#,
                params![owner, plugin_id],
            )?;
            let deleted = n > 0;
            tracing::trace!(
                "Plugin deletion result: owner={}, plugin_id={}, deleted={}",
                owner,
                plugin_id,
                deleted
            );
            Ok(deleted)
        })
        .await?
    }

    /// Lists all plugin records in the database.
    ///
    /// Returns plugins ordered by date_added_utc in descending order
    /// (most recent first).
    ///
    /// # Returns
    ///
    /// - `Ok(records)` - Vector of all plugin records
    /// - `Err(...)` if database operation fails
    pub async fn list_plugins_async(&self) -> Result<Vec<PluginRecord>> {
        tracing::trace!("Listing all plugins");
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<Vec<PluginRecord>> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            let mut stmt = conn.prepare(
                r#"SELECT owner, plugin_id, plugin_name, plugin_path, metadata, date_added_utc, plugin_data FROM plugins ORDER BY date_added_utc DESC"#,
            )?;
            tracing::trace!("Executing SQL: SELECT FROM plugins ORDER BY date_added_utc DESC");
            let mut out = Vec::new();
            let mut rows = stmt.query([])?;

            while let Some(row) = rows.next()? {
                let owner: String = row.get(0)?;
                let plugin_id: String = row.get(1)?;
                let plugin_name: Option<String> = row.get(2).ok();
                let plugin_path: Option<String> = row.get(3).ok();
                let metadata_json: String = row.get(4)?;
                let date_added_utc_str: String = row.get(5)?;
                let plugin_data: Option<Vec<u8>> = row.get(6).ok();

                match PluginRecord::from_db_row(
                    owner.clone(),
                    plugin_id.clone(),
                    plugin_name,
                    plugin_path,
                    metadata_json,
                    date_added_utc_str,
                    plugin_data,
                ) {
                    Ok(rec) => out.push(rec),
                    Err(e) => tracing::warn!(error=%e, "Skipping malformed plugin row`{}`", plugin_id),
                }
            }
            tracing::trace!("Listed {} plugins", out.len());
            Ok(out)
        })
        .await?
    }

    /// Lists all plugin records for a specific owner.
    ///
    /// Returns plugins ordered by date_added_utc in descending order
    /// (most recent first).
    ///
    /// # Arguments
    ///
    /// * `owner` - Plugin owner identifier to filter by
    ///
    /// # Returns
    ///
    /// - `Ok(records)` - Vector of plugin records for the owner
    /// - `Err(...)` if database operation fails
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Database connection fails
    /// - JSON deserialization fails
    /// - Date parsing fails
    // Used by integration tests; keep public for test access and suppress
    // editor-only dead-code warnings.
    #[allow(dead_code)]
    pub async fn list_plugins_by_owner_async(&self, owner: String) -> Result<Vec<PluginRecord>> {
        tracing::trace!("Listing plugins by owner: owner={}", owner);
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || -> Result<Vec<PluginRecord>> {
            let conn = Connection::open(&db_path)
                .with_context(|| format!("opening sqlite db at {}", db_path.display()))?;

            let mut stmt = conn.prepare(
                r#"SELECT owner, plugin_id, plugin_name, plugin_path, metadata, date_added_utc, plugin_data FROM plugins WHERE owner = ?1 ORDER BY date_added_utc DESC"#,
            )?;
            tracing::trace!("Executing SQL: SELECT FROM plugins WHERE owner = {} ORDER BY date_added_utc DESC", owner);
            let mut out = Vec::new();
            let mut rows = stmt.query(params![owner])?;

            while let Some(row) = rows.next()? {
                let owner: String = row.get(0)?;
                let plugin_id: String = row.get(1)?;
                let plugin_name: Option<String> = row.get(2).ok();
                let plugin_path: Option<String> = row.get(3).ok();
                let metadata_json: String = row.get(4)?;
                let date_added_utc_str: String = row.get(5)?;
                let plugin_data: Option<Vec<u8>> = row.get(6).ok();

                match PluginRecord::from_db_row(
                    owner.clone(),
                    plugin_id.clone(),
                    plugin_name,
                    plugin_path,
                    metadata_json,
                    date_added_utc_str,
                    plugin_data,
                ) {
                    Ok(rec) => out.push(rec),
                    Err(e) => tracing::warn!(error=%e, "Skipping malformed plugin row: {}/{}", owner, plugin_id),
                }
            }
            tracing::trace!("Listed {} plugins for owner: {}", out.len(), owner);
            Ok(out)
        })
        .await?
    }
}

/// Resolves the default database file path.
///
/// Checks for the `ARK_DB_PATH` environment variable first, then falls back
/// to platform-specific default locations:
///
/// - Windows: `%PROGRAMDATA%\ark\ark.db`
/// - Unix/Linux: `/var/ark/ark.db`
///
/// # Returns
///
/// The resolved database file path.
fn resolve_db_path() -> Result<PathBuf> {
    if let Ok(p) = env::var("ARK_DB_PATH") {
        return Ok(PathBuf::from(p));
    }

    #[cfg(target_os = "windows")]
    {
        let program_data =
            env::var("PROGRAMDATA").unwrap_or_else(|_| r"C:\ProgramData".to_string());
        Ok(Path::new(&program_data).join("ark").join("ark.db"))
    }

    #[cfg(not(target_os = "windows"))]
    {
        return Ok(PathBuf::from("/var/ark/ark.db"));
    }
}

/// Ensures the parent directory of the given path exists with secure permissions.
///
/// Creates all parent directories if they don't exist and sets secure
/// permissions using the utility functions.
///
/// # Arguments
///
/// * `path` - The file path whose parent directory should be ensured
///
/// # Returns
///
/// `Ok(())` if the directory exists or was created successfully.
///
/// # Errors
///
/// Returns an error if directory creation or permission setting fails.
fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating parent dir {}", parent.display()))?;

        // Quick writability check: try to create and remove a temp file to
        // ensure we have permission to write into this directory. This gives
        // a clearer error earlier than relying on SQLite errors such as
        // "attempt to write a readonly database".
        let test_file = parent.join(".ark_write_test");
        match std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&test_file)
        {
            Ok(mut f) => {
                use std::io::Write;
                // try write and flush a small payload then remove file
                if let Err(e) = f.write_all(b"ok") {
                    let _ = std::fs::remove_file(&test_file);
                    return Err(anyhow::anyhow!(
                        "parent dir not writable {}: {}",
                        parent.display(),
                        e
                    ));
                }
                let _ = std::fs::remove_file(&test_file);
            }
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "parent dir not writable {}: {}",
                    parent.display(),
                    e
                ));
            }
        }

        // Set secure permissions on the ark directory. Failures here are
        // non-fatal for usability in some environments, but we return the
        // error so callers can decide. The caller (e.g., main) will log and
        // continue without persistence if permission hardening fails.
        if let Err(e) = set_secure_dir_permissions(parent) {
            return Err(anyhow::anyhow!(
                "setting secure permissions on {}: {}",
                parent.display(),
                e
            ));
        }
    }
    Ok(())
}
