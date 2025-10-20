//! Integration tests for the persist module.
//!
//! These tests verify the database functionality including:
//! - Session management (CRUD operations, expiry, cleanup)
//! - Plugin metadata storage (CRUD operations, ownership)
//! - Database initialization and migrations
//! - Error handling and edge cases
//! - Concurrent access scenarios

use anyhow::Result;
use ark::server::auth::{Principal, ProviderKind};
use ark::server::persist::Database;
use ark::server::persist::{PluginRecord, SessionRecord};
use ark::server::roles::Role;
use chrono::Utc;
use serde_json::json;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tempfile::TempDir;
use tokio::time::sleep;

/// Helper function to create a test database in a temporary directory.
async fn create_test_database() -> Result<(Database, TempDir)> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("test.db");
    let database = Database::with_path(&db_path)?;
    Ok((database, temp_dir))
}

/// Helper function to create a test principal.
fn create_test_principal(subject: &str, provider: &str) -> Principal {
    Principal {
        subject: subject.to_string(),
        email: Some(format!("{}@example.com", subject)),
        name: Some(format!("Test User {}", subject)),
        picture: None,
        provider: provider.to_string(),
        provider_kind: ProviderKind::Oidc,
        tenant_id: None,
        oid: None,
        roles: vec![Role::User],
        is_admin: false,
        groups: vec![],
    }
}

#[tokio::test]
async fn test_database_initialization() -> Result<()> {
    let (_db, _temp_dir) = create_test_database().await?;
    // If we got here without panicking, initialization worked
    Ok(())
}

#[tokio::test]
async fn test_database_with_path() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("custom.db");

    let database = Database::with_path(&db_path)?;

    // Verify the database file was created
    assert!(db_path.exists());

    // Verify we can perform basic operations
    let principal = create_test_principal("test_user", "test_provider");
    let session_id = "test_session_123".to_string();
    let ttl = Duration::from_secs(3600);

    let expiry_system_time = SystemTime::now()
        .checked_add(ttl)
        .unwrap_or(SystemTime::now());
    let expiry_epoch = expiry_system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
    let session_record = SessionRecord {
        session_id: session_id.clone(),
        principal: principal.clone(),
        expiry_utc,
        expiry_epoch,
        is_admin: principal.is_admin,
    };
    database.save_session_record_async(session_record).await?;
    let result = database.get_session_record_async(session_id).await?;

    assert!(result.is_some());
    let rec = result.unwrap();
    assert_eq!(rec.principal.subject, principal.subject);

    Ok(())
}

#[tokio::test]
async fn test_session_save_and_retrieve() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let principal = create_test_principal("alice", "google");
    let session_id = "session_alice_123".to_string();
    let ttl = Duration::from_secs(3600);

    // Save session
    let expiry_system_time = SystemTime::now()
        .checked_add(ttl)
        .unwrap_or(SystemTime::now());
    let expiry_epoch = expiry_system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
    let session_record = SessionRecord {
        session_id: session_id.clone(),
        principal: principal.clone(),
        expiry_utc,
        expiry_epoch,
        is_admin: principal.is_admin,
    };
    database.save_session_record_async(session_record).await?;

    // Retrieve session
    let result = database
        .get_session_record_async(session_id.clone())
        .await?;
    assert!(result.is_some());

    let rec = result.unwrap();
    let retrieved_principal = rec.principal.clone();
    assert_eq!(retrieved_principal.subject, principal.subject);
    assert_eq!(retrieved_principal.email, principal.email);
    assert_eq!(retrieved_principal.name, principal.name);
    assert_eq!(retrieved_principal.provider, principal.provider);
    assert_eq!(retrieved_principal.provider_kind, principal.provider_kind);

    // Verify expiry is in the future
    assert!(rec.expiry_utc > Utc::now());

    Ok(())
}

#[tokio::test]
async fn test_session_update() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let principal1 = create_test_principal("alice", "google");
    let principal2 = create_test_principal("alice_updated", "microsoft");
    let session_id = "session_update_test".to_string();
    let ttl = Duration::from_secs(3600);

    // Save initial session
    let expiry_system_time = SystemTime::now()
        .checked_add(ttl)
        .unwrap_or(SystemTime::now());
    let expiry_epoch = expiry_system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
    database
        .save_session_record_async(SessionRecord {
            session_id: session_id.clone(),
            principal: principal1,
            expiry_utc,
            expiry_epoch,
            is_admin: false,
        })
        .await?;

    // Update session with new principal
    let expiry_system_time = SystemTime::now()
        .checked_add(ttl)
        .unwrap_or(SystemTime::now());
    let expiry_epoch = expiry_system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
    database
        .save_session_record_async(SessionRecord {
            session_id: session_id.clone(),
            principal: principal2.clone(),
            expiry_utc,
            expiry_epoch,
            is_admin: principal2.is_admin,
        })
        .await?;

    // Retrieve and verify updated session
    let result = database.get_session_record_async(session_id).await?;
    assert!(result.is_some());

    let rec = result.unwrap();
    assert_eq!(rec.principal.subject, principal2.subject);
    assert_eq!(rec.principal.provider, principal2.provider);

    Ok(())
}

#[tokio::test]
async fn test_session_delete() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let principal = create_test_principal("bob", "entra");
    let session_id = "session_bob_456".to_string();
    let ttl = Duration::from_secs(3600);

    // Save session
    let expiry_system_time = SystemTime::now()
        .checked_add(ttl)
        .unwrap_or(SystemTime::now());
    let expiry_epoch = expiry_system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
    database
        .save_session_record_async(SessionRecord {
            session_id: session_id.clone(),
            principal: principal.clone(),
            expiry_utc,
            expiry_epoch,
            is_admin: false,
        })
        .await?;

    // Verify session exists
    let result = database
        .get_session_record_async(session_id.clone())
        .await?;
    assert!(result.is_some());

    // Delete session
    let deleted = database.delete_session_async(session_id.clone()).await?;
    assert!(deleted);

    // Verify session is gone
    let result = database
        .get_session_record_async(session_id.clone())
        .await?;
    assert!(result.is_none());

    // Try to delete again - should return false
    let deleted_again = database.delete_session_async(session_id).await?;
    assert!(!deleted_again);

    Ok(())
}

#[tokio::test]
async fn test_session_expiry() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let principal = create_test_principal("charlie", "test_provider");
    let session_id = "session_charlie_expired".to_string();
    let ttl = Duration::from_millis(100); // Very short TTL

    // Save session with short TTL
    database
        .save_session_record_async({
            let expiry_system_time = SystemTime::now()
                .checked_add(ttl)
                .unwrap_or(SystemTime::now());
            let expiry_epoch = expiry_system_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
            SessionRecord {
                session_id: session_id.clone(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: false,
            }
        })
        .await?;

    // Wait for expiry
    sleep(Duration::from_millis(200)).await;

    // Verify session is still in database but expired
    let result = database.get_session_record_async(session_id).await?;
    assert!(result.is_some());

    let rec = result.unwrap();
    assert!(rec.expiry_utc < Utc::now()); // Should be expired

    Ok(())
}

#[tokio::test]
async fn test_session_cleanup() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let principal1 = create_test_principal("user1", "provider1");
    let principal2 = create_test_principal("user2", "provider2");
    let principal3 = create_test_principal("user3", "provider3");

    // Create sessions with different expiry times
    let expired_ttl = Duration::from_millis(50);
    let valid_ttl = Duration::from_secs(3600);

    // Save as SessionRecord instances
    for (sid, ttl_val, principal) in &[
        ("expired_session_1", expired_ttl, principal1),
        ("expired_session_2", expired_ttl, principal2),
    ] {
        let expiry_system_time = SystemTime::now()
            .checked_add(*ttl_val)
            .unwrap_or(SystemTime::now());
        let expiry_epoch = expiry_system_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
        database
            .save_session_record_async(SessionRecord {
                session_id: sid.to_string(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: principal.is_admin,
            })
            .await?;
    }
    // Valid session
    let expiry_system_time = SystemTime::now()
        .checked_add(valid_ttl)
        .unwrap_or(SystemTime::now());
    let expiry_epoch = expiry_system_time
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
    database
        .save_session_record_async(SessionRecord {
            session_id: "valid_session".to_string(),
            principal: principal3,
            expiry_utc,
            expiry_epoch,
            is_admin: false,
        })
        .await?;

    // Wait for some sessions to expire
    sleep(Duration::from_millis(100)).await;

    // Run cleanup
    let cleaned_count = database.cleanup_expired_sessions_async().await?;
    assert_eq!(cleaned_count, 2); // Should clean up 2 expired sessions

    // Verify expired sessions are gone
    let result1 = database
        .get_session_record_async("expired_session_1".to_string())
        .await?;
    let result2 = database
        .get_session_record_async("expired_session_2".to_string())
        .await?;
    assert!(result1.is_none());
    assert!(result2.is_none());

    // Verify valid session still exists
    let result3 = database
        .get_session_record_async("valid_session".to_string())
        .await?;
    assert!(result3.is_some());

    Ok(())
}

#[tokio::test]
async fn test_multiple_sessions() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let ttl = Duration::from_secs(3600);
    let mut session_ids = Vec::new();

    // Create multiple sessions
    for i in 0..10 {
        let principal = create_test_principal(&format!("user_{}", i), "test_provider");
        let session_id = format!("session_{}", i);
        let expiry_system_time = SystemTime::now()
            .checked_add(ttl)
            .unwrap_or(SystemTime::now());
        let expiry_epoch = expiry_system_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);

        database
            .save_session_record_async(SessionRecord {
                session_id: session_id.clone(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: false,
            })
            .await?;
        session_ids.push(session_id);
    }

    // Verify all sessions exist
    for session_id in &session_ids {
        let result = database
            .get_session_record_async(session_id.clone())
            .await?;
        assert!(result.is_some());
    }

    // Delete some sessions
    for session_id in session_ids.iter().take(5) {
        let deleted = database.delete_session_async(session_id.clone()).await?;
        assert!(deleted);
    }

    // Verify correct sessions are gone
    for (i, session_id) in session_ids.iter().enumerate() {
        let result = database
            .get_session_record_async(session_id.clone())
            .await?;
        if i < 5 {
            assert!(result.is_none()); // Should be deleted
        } else {
            assert!(result.is_some()); // Should still exist
        }
    }

    Ok(())
}

#[tokio::test]
async fn test_plugin_upsert_and_retrieve() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let owner = "user:tenant:123".to_string();
    let plugin_id = "test_plugin".to_string();
    let metadata = json!({
        "name": "Test Plugin",
        "version": "1.0.0",
        "description": "A test plugin",
        "tools": ["tool1", "tool2"]
    });

    // Upsert plugin via model-based writer
    database
        .save_plugin_record_async(PluginRecord {
            owner: owner.clone(),
            plugin_id: plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata: metadata.clone(),
            date_added_utc: chrono::Utc::now(),
        })
        .await?;

    // Retrieve plugin
    let result = database
        .get_plugin_async(owner.clone(), plugin_id.clone())
        .await?;
    assert!(result.is_some());

    let plugin_record = result.unwrap();
    assert_eq!(plugin_record.owner, owner);
    assert_eq!(plugin_record.plugin_id, plugin_id);
    assert_eq!(plugin_record.metadata, metadata);

    Ok(())
}

#[tokio::test]
async fn test_plugin_update() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let owner = "user:tenant:456".to_string();
    let plugin_id = "updatable_plugin".to_string();
    let metadata1 = json!({"version": "1.0.0"});
    let metadata2 = json!({"version": "2.0.0", "new_field": "added"});

    // Insert initial plugin
    database
        .save_plugin_record_async(PluginRecord {
            owner: owner.clone(),
            plugin_id: plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata: metadata1,
            date_added_utc: chrono::Utc::now(),
        })
        .await?;

    let initial_record = database
        .get_plugin_async(owner.clone(), plugin_id.clone())
        .await?
        .unwrap();
    let initial_date = initial_record.date_added_utc;

    // Wait a bit to ensure timestamp difference
    sleep(Duration::from_millis(10)).await;

    // Update plugin
    database
        .save_plugin_record_async(PluginRecord {
            owner: owner.clone(),
            plugin_id: plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata: metadata2.clone(),
            date_added_utc: chrono::Utc::now(),
        })
        .await?;

    // Retrieve updated plugin
    let updated_record = database
        .get_plugin_async(owner.clone(), plugin_id.clone())
        .await?
        .unwrap();
    assert_eq!(updated_record.metadata, metadata2);

    // Verify timestamp was updated
    assert!(updated_record.date_added_utc >= initial_date);

    Ok(())
}

#[tokio::test]
async fn test_plugin_delete() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let owner = "user:tenant:789".to_string();
    let plugin_id = "deletable_plugin".to_string();
    let metadata = json!({"test": "data"});

    // Insert plugin via model-based writer
    database
        .save_plugin_record_async(PluginRecord {
            owner: owner.clone(),
            plugin_id: plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata,
            date_added_utc: chrono::Utc::now(),
        })
        .await?;

    // Verify plugin exists
    let result = database
        .get_plugin_async(owner.clone(), plugin_id.clone())
        .await?;
    assert!(result.is_some());

    // Delete plugin
    let deleted = database
        .delete_plugin_async(owner.clone(), plugin_id.clone())
        .await?;
    assert!(deleted);

    // Verify plugin is gone
    let result = database
        .get_plugin_async(owner.clone(), plugin_id.clone())
        .await?;
    assert!(result.is_none());

    // Try to delete again - should return false
    let deleted_again = database.delete_plugin_async(owner, plugin_id).await?;
    assert!(!deleted_again);

    Ok(())
}

#[tokio::test]
async fn test_plugin_list_all() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Insert multiple plugins for different owners
    let plugins = vec![
        ("owner1", "plugin_a", json!({"name": "Plugin A"})),
        ("owner1", "plugin_b", json!({"name": "Plugin B"})),
        ("owner2", "plugin_c", json!({"name": "Plugin C"})),
        ("owner2", "plugin_d", json!({"name": "Plugin D"})),
        ("owner3", "plugin_e", json!({"name": "Plugin E"})),
    ];

    for (owner, plugin_id, metadata) in &plugins {
        database
            .save_plugin_record_async(PluginRecord {
                owner: owner.to_string(),
                plugin_id: plugin_id.to_string(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: metadata.clone(),
                date_added_utc: chrono::Utc::now(),
            })
            .await?;
    }

    // List all plugins
    let all_plugins = database.list_plugins_async().await?;
    assert_eq!(all_plugins.len(), 5);

    // Verify plugins are ordered by date_added_utc (most recent first)
    for i in 1..all_plugins.len() {
        assert!(all_plugins[i - 1].date_added_utc >= all_plugins[i].date_added_utc);
    }

    Ok(())
}

#[tokio::test]
async fn test_plugin_list_by_owner() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Insert plugins for different owners
    let owner1_plugins = vec![
        ("plugin_1", json!({"owner1": "data1"})),
        ("plugin_2", json!({"owner1": "data2"})),
    ];

    let owner2_plugins = vec![("plugin_3", json!({"owner2": "data3"}))];

    for (plugin_id, metadata) in &owner1_plugins {
        database
            .save_plugin_record_async(PluginRecord {
                owner: "owner1".to_string(),
                plugin_id: plugin_id.to_string(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: metadata.clone(),
                date_added_utc: chrono::Utc::now(),
            })
            .await?;
    }

    for (plugin_id, metadata) in &owner2_plugins {
        database
            .save_plugin_record_async(PluginRecord {
                owner: "owner2".to_string(),
                plugin_id: plugin_id.to_string(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: metadata.clone(),
                date_added_utc: chrono::Utc::now(),
            })
            .await?;
    }

    // List plugins by owner1
    let owner1_list = database
        .list_plugins_by_owner_async("owner1".to_string())
        .await?;
    assert_eq!(owner1_list.len(), 2);
    for plugin in &owner1_list {
        assert_eq!(plugin.owner, "owner1");
    }

    // List plugins by owner2
    let owner2_list = database
        .list_plugins_by_owner_async("owner2".to_string())
        .await?;
    assert_eq!(owner2_list.len(), 1);
    assert_eq!(owner2_list[0].owner, "owner2");

    // List plugins by non-existent owner
    let empty_list = database
        .list_plugins_by_owner_async("nonexistent".to_string())
        .await?;
    assert_eq!(empty_list.len(), 0);

    Ok(())
}

#[tokio::test]
async fn test_concurrent_session_operations() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let ttl = Duration::from_secs(3600);
    let num_concurrent = 10;

    // Create concurrent tasks for session operations
    let mut handles = Vec::new();

    for i in 0..num_concurrent {
        let db = database.clone();
        let principal = create_test_principal(&format!("concurrent_user_{}", i), "test_provider");
        let session_id = format!("concurrent_session_{}", i);

        let handle = tokio::spawn(async move {
            // Save session
            db.save_session_record_async({
                let expiry_system_time = SystemTime::now()
                    .checked_add(ttl)
                    .unwrap_or(SystemTime::now());
                let expiry_epoch = expiry_system_time
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
                SessionRecord {
                    session_id: session_id.clone(),
                    principal: principal.clone(),
                    expiry_utc,
                    expiry_epoch,
                    is_admin: false,
                }
            })
            .await?;

            // Retrieve session
            let result = db.get_session_record_async(session_id.clone()).await?;
            assert!(result.is_some());

            // Delete session
            let deleted = db.delete_session_async(session_id).await?;
            assert!(deleted);

            Ok::<(), anyhow::Error>(())
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await??;
    }

    Ok(())
}

#[tokio::test]
async fn test_concurrent_plugin_operations() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let num_concurrent = 10;

    // Create concurrent tasks for plugin operations
    let mut handles = Vec::new();

    for i in 0..num_concurrent {
        let db = database.clone();
        let owner = format!("concurrent_owner_{}", i);
        let plugin_id = format!("concurrent_plugin_{}", i);
        let metadata = json!({"index": i, "data": "concurrent_test"});

        let handle = tokio::spawn(async move {
            // Upsert plugin via model-based writer
            db.save_plugin_record_async(PluginRecord {
                owner: owner.clone(),
                plugin_id: plugin_id.clone(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: metadata.clone(),
                date_added_utc: chrono::Utc::now(),
            })
            .await?;

            // Retrieve plugin
            let result = db
                .get_plugin_async(owner.clone(), plugin_id.clone())
                .await?;
            assert!(result.is_some());

            // Update plugin
            let updated_metadata = json!({"index": i, "data": "updated", "version": 2});
            db.save_plugin_record_async(PluginRecord {
                owner: owner.clone(),
                plugin_id: plugin_id.clone(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: updated_metadata,
                date_added_utc: chrono::Utc::now(),
            })
            .await?;

            // Delete plugin
            let deleted = db.delete_plugin_async(owner, plugin_id).await?;
            assert!(deleted);

            Ok::<(), anyhow::Error>(())
        });

        handles.push(handle);
    }

    // Wait for all tasks to complete
    for handle in handles {
        handle.await??;
    }

    Ok(())
}

#[tokio::test]
async fn test_error_handling_nonexistent_session() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Try to get non-existent session
    let result = database
        .get_session_record_async("nonexistent_session".to_string())
        .await?;
    assert!(result.is_none());

    // Try to delete non-existent session
    let deleted = database
        .delete_session_async("nonexistent_session".to_string())
        .await?;
    assert!(!deleted);

    Ok(())
}

#[tokio::test]
async fn test_error_handling_nonexistent_plugin() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Try to get non-existent plugin
    let result = database
        .get_plugin_async(
            "nonexistent_owner".to_string(),
            "nonexistent_plugin".to_string(),
        )
        .await?;
    assert!(result.is_none());

    // Try to delete non-existent plugin
    let deleted = database
        .delete_plugin_async(
            "nonexistent_owner".to_string(),
            "nonexistent_plugin".to_string(),
        )
        .await?;
    assert!(!deleted);

    Ok(())
}

#[tokio::test]
async fn test_large_metadata_storage() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Create large metadata object
    let mut large_metadata = json!({
        "name": "Large Plugin",
        "description": "A plugin with large metadata"
    });

    // Add large array of data
    let large_array: Vec<serde_json::Value> = (0..1000)
        .map(|i| json!({"id": i, "name": format!("item_{}", i), "data": "x".repeat(100)}))
        .collect();

    large_metadata["large_data"] = json!(large_array);

    let owner = "large_data_owner".to_string();
    let plugin_id = "large_plugin".to_string();

    // Store and retrieve large metadata via model-based writer
    database
        .save_plugin_record_async(PluginRecord {
            owner: owner.clone(),
            plugin_id: plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata: large_metadata.clone(),
            date_added_utc: chrono::Utc::now(),
        })
        .await?;

    let result = database.get_plugin_async(owner, plugin_id).await?;
    assert!(result.is_some());

    let plugin_record = result.unwrap();
    assert_eq!(plugin_record.metadata, large_metadata);

    Ok(())
}

#[tokio::test]
async fn test_special_characters_in_data() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Test session with special characters
    let mut principal = create_test_principal("user🚀", "provider💯");
    principal.email = Some("test+email@example.com".to_string());
    principal.name = Some("User Name with 'quotes' and \"double quotes\"".to_string());

    let session_id = "session_special_chars_😀🔥".to_string();
    let ttl = Duration::from_secs(3600);

    database
        .save_session_record_async({
            let expiry_system_time = SystemTime::now()
                .checked_add(ttl)
                .unwrap_or(SystemTime::now());
            let expiry_epoch = expiry_system_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
            SessionRecord {
                session_id: session_id.clone(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: principal.is_admin,
            }
        })
        .await?;

    let result = database.get_session_record_async(session_id).await?;
    assert!(result.is_some());

    let rec = result.unwrap();
    assert_eq!(rec.principal.subject, principal.subject);
    assert_eq!(rec.principal.email, principal.email);
    assert_eq!(rec.principal.name, principal.name);

    // Test plugin with special characters
    let owner = "owner/with\\special:chars".to_string();
    let plugin_id = "plugin-with-dashes_and_underscores".to_string();
    let metadata = json!({
        "name": "Plugin with 'quotes' and \"double quotes\"",
        "special_chars": "🚀💯🔥😀",
        "unicode": "こんにちは世界",
        "sql_injection_attempt": "'; DROP TABLE sessions; --"
    });

    database
        .save_plugin_record_async(PluginRecord {
            owner: owner.clone(),
            plugin_id: plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata: metadata.clone(),
            date_added_utc: chrono::Utc::now(),
        })
        .await?;

    let result = database.get_plugin_async(owner, plugin_id).await?;
    assert!(result.is_some());

    let plugin_record = result.unwrap();
    assert_eq!(plugin_record.metadata, metadata);

    Ok(())
}

#[tokio::test]
async fn test_session_zero_ttl() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let principal = create_test_principal("zero_ttl_user", "test_provider");
    let session_id = "zero_ttl_session".to_string();
    let ttl = Duration::from_secs(0); // Zero TTL

    // Save session with zero TTL
    database
        .save_session_record_async({
            let expiry_system_time = SystemTime::now()
                .checked_add(ttl)
                .unwrap_or(SystemTime::now());
            let expiry_epoch = expiry_system_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
            SessionRecord {
                session_id: session_id.clone(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: principal.is_admin,
            }
        })
        .await?;

    // Retrieve session - should exist but be immediately expired
    let result = database.get_session_record_async(session_id).await?;
    assert!(result.is_some());

    let rec = result.unwrap();
    assert!(rec.expiry_utc <= Utc::now()); // Should be expired immediately

    Ok(())
}

#[tokio::test]
async fn test_empty_string_identifiers() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    // Test session with empty string ID
    let principal = create_test_principal("user", "provider");
    let empty_session_id = "".to_string();
    let ttl = Duration::from_secs(3600);

    // Should handle empty session ID gracefully
    database
        .save_session_record_async({
            let expiry_system_time = SystemTime::now()
                .checked_add(ttl)
                .unwrap_or(SystemTime::now());
            let expiry_epoch = expiry_system_time
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
            SessionRecord {
                session_id: empty_session_id.clone(),
                principal: principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: principal.is_admin,
            }
        })
        .await?;
    let result = database.get_session_record_async(empty_session_id).await?;
    assert!(result.is_some());

    // Test plugin with empty strings
    let empty_owner = "".to_string();
    let empty_plugin_id = "".to_string();
    let metadata = json!({"test": "empty_ids"});

    database
        .save_plugin_record_async(PluginRecord {
            owner: empty_owner.clone(),
            plugin_id: empty_plugin_id.clone(),
            plugin_name: None,
            plugin_path: None,
            plugin_data: None,
            metadata: metadata.clone(),
            date_added_utc: chrono::Utc::now(),
        })
        .await?;
    let result = database
        .get_plugin_async(empty_owner, empty_plugin_id)
        .await?;
    assert!(result.is_some());

    Ok(())
}

#[tokio::test]
async fn test_plugin_metadata_types() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let owner = "metadata_test_owner".to_string();

    // Test different JSON value types
    let test_cases = vec![
        ("null_plugin", json!(null)),
        ("bool_plugin", json!(true)),
        ("number_plugin", json!(42)),
        ("string_plugin", json!("simple string")),
        ("array_plugin", json!([1, 2, 3, "mixed", true, null])),
        (
            "object_plugin",
            json!({"nested": {"deeply": {"nested": "value"}}}),
        ),
        ("empty_object_plugin", json!({})),
        ("empty_array_plugin", json!([])),
    ];

    for (plugin_id, metadata) in test_cases {
        database
            .save_plugin_record_async(PluginRecord {
                owner: owner.clone(),
                plugin_id: plugin_id.to_string(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: metadata.clone(),
                date_added_utc: chrono::Utc::now(),
            })
            .await?;

        let result = database
            .get_plugin_async(owner.clone(), plugin_id.to_string())
            .await?;
        assert!(result.is_some());

        let plugin_record = result.unwrap();
        assert_eq!(plugin_record.metadata, metadata);
    }

    Ok(())
}

#[tokio::test]
async fn test_database_persistence_across_instances() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let db_path = temp_dir.path().join("persistence_test.db");

    let principal = create_test_principal("persistent_user", "test_provider");
    let session_id = "persistent_session".to_string();
    let ttl = Duration::from_secs(3600);

    // Create first database instance and store data
    {
        let database1 = Database::with_path(&db_path)?;
        database1
            .save_session_record_async({
                let expiry_system_time = SystemTime::now()
                    .checked_add(ttl)
                    .unwrap_or(SystemTime::now());
                let expiry_epoch = expiry_system_time
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64;
                let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
                SessionRecord {
                    session_id: session_id.clone(),
                    principal: principal.clone(),
                    expiry_utc,
                    expiry_epoch,
                    is_admin: principal.is_admin,
                }
            })
            .await?;

        let owner = "persistent_owner".to_string();
        let plugin_id = "persistent_plugin".to_string();
        let metadata = json!({"persisted": true});
        database1
            .save_plugin_record_async(PluginRecord {
                owner: owner.clone(),
                plugin_id: plugin_id.clone(),
                plugin_name: None,
                plugin_path: None,
                plugin_data: None,
                metadata: metadata.clone(),
                date_added_utc: chrono::Utc::now(),
            })
            .await?;

        // database1 is dropped here
    }

    // Create second database instance and verify data persists
    {
        let database2 = Database::with_path(&db_path)?;

        // Verify session persists
        let session_result = database2.get_session_record_async(session_id).await?;
        assert!(session_result.is_some());
        let rec = session_result.unwrap();
        assert_eq!(rec.principal.subject, principal.subject);

        // Verify plugin persists
        let plugin_result = database2
            .get_plugin_async(
                "persistent_owner".to_string(),
                "persistent_plugin".to_string(),
            )
            .await?;
        assert!(plugin_result.is_some());
        let plugin_record = plugin_result.unwrap();
        assert_eq!(plugin_record.metadata["persisted"], json!(true));
    }

    Ok(())
}

#[tokio::test]
async fn test_cleanup_with_mixed_expiry_times() -> Result<()> {
    let (database, _temp_dir) = create_test_database().await?;

    let base_principal = create_test_principal("cleanup_user", "test_provider");

    // Create sessions with varied expiry times
    let sessions = vec![
        ("expired_1ms", Duration::from_millis(1)),
        ("expired_2ms", Duration::from_millis(2)),
        ("valid_1hour", Duration::from_secs(3600)),
        ("expired_3ms", Duration::from_millis(3)),
        ("valid_2hours", Duration::from_secs(7200)),
        ("expired_4ms", Duration::from_millis(4)),
    ];

    for (session_id, ttl) in &sessions {
        let expiry_system_time = SystemTime::now()
            .checked_add(*ttl)
            .unwrap_or(SystemTime::now());
        let expiry_epoch = expiry_system_time
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);
        database
            .save_session_record_async(SessionRecord {
                session_id: session_id.to_string(),
                principal: base_principal.clone(),
                expiry_utc,
                expiry_epoch,
                is_admin: base_principal.is_admin,
            })
            .await?;
    }

    // Wait for short TTL sessions to expire
    sleep(Duration::from_millis(10)).await;

    // Run cleanup
    let cleaned_count = database.cleanup_expired_sessions_async().await?;
    assert_eq!(cleaned_count, 4); // Should clean up the 4 expired sessions

    // Verify only valid sessions remain
    for (session_id, ttl) in &sessions {
        let result = database
            .get_session_record_async(session_id.to_string())
            .await?;
        if ttl.as_secs() >= 3600 {
            assert!(
                result.is_some(),
                "Valid session {} should still exist",
                session_id
            );
        } else {
            assert!(
                result.is_none(),
                "Expired session {} should be cleaned up",
                session_id
            );
        }
    }

    Ok(())
}
