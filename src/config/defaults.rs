/// Ark configuration defaults module.
///
/// This module provides default value helpers for serde deserialization
/// in config structs. These functions ensure consistent defaulting across
/// `models.rs` and `mod.rs`, and handle cases where entire config blocks
/// are missing from the configuration file.
use super::models::{ManagementPathConfig, McpTransport};

/// Default MCP transport.
///
/// Returns `Some(McpTransport::Stdio)` as the default transport.
pub(crate) fn default_transport() -> Option<McpTransport> {
    Some(McpTransport::Stdio)
}

/// Default CORS configuration.
///
/// Returns `None` to indicate no CORS configuration.
pub(crate) fn default_cors() -> Option<String> {
    None
}

/// Default management server bind address.
///
/// Returns the constant `DEFAULT_MGMT_BIND_ADDRESS`.
pub(crate) fn default_mgmt_bind_address() -> String {
    crate::server::constants::DEFAULT_MGMT_BIND_ADDRESS.to_string()
}

/// Default management server bind address as an option.
///
/// Returns `Some(default_mgmt_bind_address())`.
pub(crate) fn default_mgmt_bind_address_opt() -> Option<String> {
    Some(default_mgmt_bind_address())
}

/// Default MCP server bind address.
///
/// Returns the constant `DEFAULT_MCP_BIND_ADDRESS`.
pub(crate) fn default_mcp_bind_address() -> String {
    crate::server::constants::DEFAULT_MCP_BIND_ADDRESS.to_string()
}

/// Default MCP server bind address as an option.
///
/// Returns `Some(default_mcp_bind_address())`.
pub(crate) fn default_mcp_bind_address_opt() -> Option<String> {
    Some(default_mcp_bind_address())
}

/// Default liveness probe path.
///
/// Returns `"/livez"`.
pub(crate) fn default_livez_path() -> String {
    "/livez".to_string()
}

/// Default readiness probe path.
///
/// Returns `"/readyz"`.
pub(crate) fn default_readyz_path() -> String {
    "/readyz".to_string()
}

/// Default true value.
///
/// Returns `true`.
pub(crate) fn default_true() -> bool {
    true
}

/// Default false value.
///
/// Returns `false`.
pub(crate) fn default_false() -> bool {
    false
}

/// Default liveness probe configuration.
///
/// Returns a `ManagementPathConfig` with `/livez` path and disabled by default.
pub(crate) fn default_livez() -> ManagementPathConfig {
    ManagementPathConfig {
        path: Some(default_livez_path()),
        enabled: false,
    }
}

/// Default readiness probe configuration.
///
/// Returns a `ManagementPathConfig` with `/readyz` path and disabled by default.
pub(crate) fn default_readyz() -> ManagementPathConfig {
    ManagementPathConfig {
        path: Some(default_readyz_path()),
        enabled: false,
    }
}

/// Default management response type.
///
/// Returns `"json"`.
pub(crate) fn default_mgmt_response_type() -> String {
    "json".to_string()
}

// ----------------- Auth / Session Defaults -----------------
pub(crate) fn default_session_timeout() -> u64 {
    3600
}
pub(crate) fn default_session_cookie_name() -> String {
    "ark_session".to_string()
}
pub(crate) fn default_cookie_same_site() -> String {
    "Lax".to_string()
}
