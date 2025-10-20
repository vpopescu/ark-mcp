use anyhow::{Context, Result};

use rusqlite::OptionalExtension;
use std::fs;
#[cfg(unix)]
use std::fs::OpenOptions;
#[cfg(unix)]
use fs2::FileExt;

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::Builder as TempBuilder;

#[test]
fn test_concurrent_migrations_file_lock() -> Result<()> {
    // The test exercises the same advisory locking code paths used in
    // production. Windows file-share semantics require opening the lock file
    // with explicit share flags; the production code uses compatible flags
    // and the test exercises that behavior here.
    // Prepare temporary directories for migrations and database
    let tmp = TempBuilder::new().prefix("ark_test_migrations").tempdir()?;
    let migrations_dir = tmp.path().join("migrations");
    let sqlite_dir = migrations_dir.join("sqlite");
    fs::create_dir_all(&sqlite_dir)?;

    // A simple migration that creates a test table
    let migration_sql = r#"CREATE TABLE IF NOT EXISTS concurrent_migration_test (
        id INTEGER PRIMARY KEY,
        name TEXT
    );"#;
    fs::write(
        sqlite_dir.join("V001__create_concurrent_test_table.sql"),
        migration_sql,
    )?;

    // Create a temporary database path
    let tmp_db_dir = TempBuilder::new().prefix("ark_test_db").tempdir()?;
    let db_path = tmp_db_dir.path().join("concurrent_test.db");

    // We'll spawn the current test binary itself to run a single helper test
    // (named `child_lock_helper`) which will acquire the lock and hold it.
    // This avoids depending on externally-provided helper binaries.
    let test_exe = std::env::current_exe().context("failed to locate current test executable")?;

    // Where the lock file will be located (same logic as Database::run_bootstrap_migrations)
    let lock_path = db_path.with_extension("migrate.lock");
    let signal_file = tmp.path().join("locker.ready");
    if signal_file.exists() {
        let _ = fs::remove_file(&signal_file);
    }

    // Spawn the locker process which acquires the migration lock and holds it
    let hold_ms: u64 = 1000; // hold long enough for the second process to attempt acquisition
    let mut locker_child = Command::new(&test_exe)
        // Instruct the test runner to execute only the child helper test
        .arg("child_lock_helper")
        .env("CHILD_LOCKER_PATH", &lock_path)
        .env("CHILD_HOLD_MS", hold_ms.to_string())
        .env("CHILD_SIGNAL_FILE", &signal_file)
        .spawn()
        .with_context(|| format!("spawning locker child via {}", test_exe.display()))?;

    // Wait for the locker to signal that it has acquired the lock
    let started = Instant::now();
    let ready_timeout = Duration::from_secs(5);
    loop {
        if signal_file.exists() {
            break;
        }
        if started.elapsed() > ready_timeout {
            // Ensure we kill the locker child to avoid leaking a process
            let _ = locker_child.kill();
            return Err(anyhow::anyhow!(
                "timed out waiting for locker child to become ready"
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    // Now spawn the migration apply child test which should block until the lock is released
    let start = Instant::now();
    let apply_output = Command::new(&test_exe)
        // Instruct the test runner to execute only the child helper which
        // applies migrations for a provided DB path.
        .arg("child_apply_migrations")
        .env("CHILD_APPLY_DB", &db_path)
        .env("ARK_MIGRATIONS_DIR", &sqlite_dir)
        .output()
        .with_context(|| format!("running migration apply child via {}", test_exe.display()))?;
    let elapsed = start.elapsed();

    // Ensure the apply process succeeded
    if !apply_output.status.success() {
        let stdout = String::from_utf8_lossy(&apply_output.stdout);
        let stderr = String::from_utf8_lossy(&apply_output.stderr);
        return Err(anyhow::anyhow!(
            "migration apply failed (status: {:?})\nstdout:\n{}\nstderr:\n{}",
            apply_output.status.code(),
            stdout,
            stderr
        ));
    }

    // The apply process should have waited at least as long as the lock hold time
    assert!(
        elapsed >= Duration::from_millis(900),
        "migration apply did not wait for the locker ({:?} < 900ms)",
        elapsed
    );

    // Verify the migration was applied by checking that the table exists
    let conn = rusqlite::Connection::open(&db_path)?;
    let mut stmt = conn.prepare(
        "SELECT name FROM sqlite_master WHERE type='table' AND name='concurrent_migration_test'",
    )?;
    let found: Option<String> = stmt.query_row([], |r| r.get(0)).optional()?;
    assert!(found.is_some(), "expected migration table to exist");

    // Ensure locker process finished cleanly
    let locker_status = locker_child
        .wait()
        .with_context(|| "waiting for locker child to exit")?;
    assert!(
        locker_status.success(),
        "locker child failed: {:?}",
        locker_status
    );

    Ok(())
}

/// Helper test executed by a spawned child test-runner. When the parent test
/// wants a separate process to acquire the migration lock, it spawns the
/// current test executable filtered to run this test only and sets the
/// following environment variables:
/// - CHILD_LOCKER_PATH: path to the lock file to acquire
/// - CHILD_HOLD_MS: milliseconds to hold the lock
/// - CHILD_SIGNAL_FILE: file to create once the lock is acquired (optional)
#[test]
fn child_lock_helper() -> Result<()> {
    // If not invoked as the child helper, simply return early so the normal
    // test run is unaffected.
    let lock_path_var = match std::env::var_os("CHILD_LOCKER_PATH") {
        Some(p) => p,
        None => return Ok(()),
    };
    let lock_path = PathBuf::from(lock_path_var);
    let hold_ms: u64 = std::env::var("CHILD_HOLD_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1000);
    let signal_file = std::env::var_os("CHILD_SIGNAL_FILE").map(PathBuf::from);

    // On Windows we use a kernel-named mutex derived from the lock path so
    // both production and test code synchronize using the same primitive.
    #[cfg(windows)]
    {
        use sha2::{Digest, Sha256};
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};
        use windows::core::PCWSTR;

        // Derived mutex name must match production's derivation.
        let mut hasher = Sha256::new();
        hasher.update(lock_path.to_string_lossy().as_bytes());
        let digest = hex::encode(hasher.finalize());
        let name = format!("Global\\ark_migrate_{}", digest);
        let wide: Vec<u16> = std::ffi::OsStr::new(&name)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let pcw = PCWSTR(wide.as_ptr());

        unsafe {
            let handle = CreateMutexW(None, false, pcw)?;
            // Wait indefinitely until we acquire the mutex so the parent
            // test can ensure we hold the lock before it launches the
            // migration application process.
            let wait = WaitForSingleObject(handle, u32::MAX);
            let wait_val: u32 = wait.0 as u32;
            if wait_val != 0 {
                let _ = CloseHandle(handle);
                return Err(anyhow::anyhow!("waiting for mutex failed: {}", wait_val));
            }

            // Signal readiness
            if let Some(sig) = signal_file.clone() {
                let _ = std::fs::write(&sig, "locked");
            }

            // Hold the mutex for the requested duration
            std::thread::sleep(Duration::from_millis(hold_ms));

            // Release and close handle
            let _ = ReleaseMutex(handle);
            let _ = CloseHandle(handle);
        }
    }
    #[cfg(not(windows))]
    {
        // Open (or create) the lock file and acquire an exclusive lock. Use the
        // same options as the production code so the behavior matches the real
        // bootstrap path under test.
        let file_res = {
            OpenOptions::new()
                .create(true)
                .read(true)
                .write(true)
                .open(&lock_path)
        };
        let file =
            file_res.with_context(|| format!("opening lock file {}", lock_path.display()))?;
        file.lock_exclusive()
            .with_context(|| format!("locking file {}", lock_path.display()))?;

        // Signal readiness so the parent test knows the lock is held
        if let Some(sig) = signal_file.clone() {
            let _ = std::fs::write(&sig, "locked");
        }

        std::thread::sleep(Duration::from_millis(hold_ms));

        // Release lock and exit
        file.unlock()
            .with_context(|| format!("unlocking file {}", lock_path.display()))?;
    }

    Ok(())
}

/// Child test that applies migrations for a provided database path. The parent
/// test spawns the current test executable filtered to this test and sets the
/// following environment variables:
/// - CHILD_APPLY_DB: path to the DB file to initialize
/// - CHILD_APPLY_MIGRATIONS_DIR: path to the migrations (filesystem) to apply
#[test]
fn child_apply_migrations() -> Result<()> {
    let db_path_os = match std::env::var_os("CHILD_APPLY_DB") {
        Some(p) => p,
        None => return Ok(()),
    };
    let db_path = PathBuf::from(db_path_os);

    // The parent process should provide ARK_MIGRATIONS_DIR for filesystem-mode
    // migration application; if it's not present the embedded migrations will
    // be used instead.

    // Initialize the Database which will cause migrations to run
    let _db = ark::server::persist::Database::with_path(&db_path)
        .with_context(|| format!("applying migrations for {}", db_path.display()))?;
    Ok(())
}
