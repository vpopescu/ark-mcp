use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, UNIX_EPOCH};

/// A plugin record stored in the database.
///
/// Represents a plugin with its metadata, ownership, and creation timestamp.
/// The combination of `owner` and `plugin_id` forms the primary key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    /// Plugin owner identifier (typically from `Principal::global_id()`).
    pub owner: String,
    /// Unique plugin identifier.
    pub plugin_id: String,
    /// Optional friendly plugin name persisted alongside the record.
    pub plugin_name: Option<String>,
    /// Optional original plugin path/URL.
    pub plugin_path: Option<String>,
    /// Optional raw plugin payload (WASM bytes) when available.
    pub plugin_data: Option<Vec<u8>>,
    /// JSON metadata associated with the plugin.
    pub metadata: serde_json::Value,
    /// UTC timestamp when the plugin was added to the database.
    pub date_added_utc: chrono::DateTime<chrono::Utc>,
}

impl PluginRecord {
    /// Construct a PluginRecord from raw database column values.
    ///
    /// This helper centralizes the parsing/validation of the `metadata`
    /// JSON string and the `date_added_utc` RFC3339 timestamp returned by
    /// the database so callers can convert DB rows into strongly-typed
    /// models with useful error context.
    pub fn from_db_row(
        owner: String,
        plugin_id: String,
        plugin_name: Option<String>,
        plugin_path: Option<String>,
        metadata_json: String,
        date_added_utc_str: String,
        plugin_data: Option<Vec<u8>>,
    ) -> Result<Self> {
        let metadata: serde_json::Value =
            serde_json::from_str(&metadata_json).context("parsing plugin metadata JSON")?;
        let date_added_utc = chrono::DateTime::parse_from_rfc3339(&date_added_utc_str)
            .context("parsing date_added_utc from DB")?
            .with_timezone(&chrono::Utc);

        Ok(PluginRecord {
            owner,
            plugin_id,
            plugin_name,
            plugin_path,
            plugin_data,
            metadata,
            date_added_utc,
        })
    }
}

/// A session record stored in the database.
//
/// Mirrors the `PluginRecord` pattern: parsing helpers centralize JSON/date
/// parsing and normalize the stored `Principal` (ensuring admin flag is
/// represented consistently in-memory).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Session identifier (matches `sessions.session_id`).
    pub session_id: String,
    /// Deserialized `Principal` stored for this session.
    pub principal: crate::server::auth::Principal,
    /// Expiration time as a chrono UTC DateTime.
    pub expiry_utc: chrono::DateTime<chrono::Utc>,
    /// Expiration as epoch seconds (convenience / stable DB value).
    pub expiry_epoch: i64,
    /// Admin flag persisted for older rows or explicit overrides.
    pub is_admin: bool,
}

impl SessionRecord {
    /// Construct a SessionRecord from raw database column values.
    ///
    /// Centralizes deserialization of the `principal_json` and normalization
    /// of the admin flag so callers can treat parsed rows uniformly.
    pub fn from_db_row(
        session_id: String,
        principal_json: String,
        expiry_epoch: i64,
        is_admin_opt: Option<i64>,
    ) -> Result<Self> {
        let mut principal: crate::server::auth::Principal =
            serde_json::from_str(&principal_json).context("parsing principal JSON from DB")?;

        let is_admin = matches!(is_admin_opt, Some(v) if v != 0);
        if is_admin && !principal.roles.contains(&crate::server::roles::Role::Admin) {
            principal.roles.push(crate::server::roles::Role::Admin);
            principal.is_admin = true;
        }

        let expiry_system_time = UNIX_EPOCH + Duration::from_secs(expiry_epoch as u64);
        let expiry_utc = chrono::DateTime::<chrono::Utc>::from(expiry_system_time);

        Ok(SessionRecord {
            session_id,
            principal,
            expiry_utc,
            expiry_epoch,
            is_admin,
        })
    }
}
