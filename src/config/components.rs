/// Ark configuration components module.
///
/// This module defines user-facing configuration structures and enums for the Ark server.
/// It provides helpers for loading, parsing, and applying configuration, serving as
/// the building blocks for the root `ArkConfig` in `mod.rs`.
use oci_client::secrets::RegistryAuth;

use super::defaults;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Logging levels supported by Ark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    /// Trace-level logging (very verbose).
    Trace,
    /// Debug-level logging (verbose).
    Debug,
    /// Info-level logging (default).
    Info,
    /// Warning-level logging.
    Warn,
    /// Error-level logging only.
    Error,
}

/// Display implementation for LogLevel.
///
/// Formats the log level as a lowercase string.
impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            LogLevel::Error => "error",
            LogLevel::Warn => "warn",
            LogLevel::Info => "info",
            LogLevel::Debug => "debug",
            LogLevel::Trace => "trace",
        };
        write!(f, "{}", s)
    }
}

/// Supported transports for MCP communications.
#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    /// Standard I/O transport.
    #[default]
    Stdio,
    /// Server-Sent Events over HTTP (not implemented).
    Sse,
    /// Streamable HTTP transport.
    StreamableHTTP,
}

/// TLS configuration for secure connections.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct TlsConfig {
    /// TLS private key (relative to configuration directory).
    pub key: Option<String>,
    /// TLS certificate (relative to configuration directory).
    pub cert: Option<String>,
    /// Whether to suppress insecure connection warnings.
    #[serde(default = "defaults::default_false")]
    pub silent_insecure: bool,
}

/// Configuration for a management endpoint (liveness/readiness).
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct ManagementPathConfig {
    /// URL path to expose (e.g., "/livez"). If `None`, the endpoint is disabled.
    pub path: Option<String>,
    /// Whether the endpoint is enabled.
    pub enabled: bool,
}

/// Management server configuration, including optional separate bind address and response type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManagementEndpointConfig {
    /// Liveness probe setup.
    #[serde(default = "defaults::default_livez")]
    pub livez: ManagementPathConfig,

    /// Readiness probe setup.
    #[serde(default = "defaults::default_readyz")]
    pub readyz: ManagementPathConfig,

    /// Response body type for management endpoints: "text" (default) or "json".
    #[serde(default = "defaults::default_mgmt_response_type")]
    pub response_type: String,

    /// Whether to disable the plugin management API.
    #[serde(default = "defaults::default_false")]
    pub disable_plugin_api: bool,

    /// Whether to disable the admin console.
    #[serde(default = "defaults::default_false")]
    pub disable_console: bool,

    /// Whether to disable the health API.
    #[serde(default = "defaults::default_false")]
    pub disable_health_api: bool,

    /// Whether to disable the prometheus api
    #[serde(default = "defaults::default_false")]
    pub disable_prometheus_api: bool,

    /// Whether to disable emiting otel metrics.
    #[serde(default = "defaults::default_true")]
    pub disable_emit_otel: bool,

    /// CORS allowed origins.
    #[serde(default = "defaults::default_cors")]
    pub cors: Option<String>,

    /// Optional bind address for the management server.
    #[serde(default = "defaults::default_mgmt_bind_address_opt")]
    pub bind_address: Option<String>,
}

impl Default for ManagementEndpointConfig {
    fn default() -> Self {
        Self {
            livez: defaults::default_livez(),
            readyz: defaults::default_readyz(),
            response_type: defaults::default_mgmt_response_type(),
            disable_plugin_api: defaults::default_false(),
            disable_console: defaults::default_false(),
            disable_health_api: defaults::default_false(),
            disable_prometheus_api: defaults::default_false(),
            disable_emit_otel: defaults::default_true(),
            cors: defaults::default_cors(),
            bind_address: defaults::default_mgmt_bind_address_opt(),
        }
    }
}

/// MCP server configuration, including optional separate bind address.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpEndpointConfig {
    /// CORS allowed origins.
    #[serde(default = "defaults::default_cors")]
    pub cors: Option<String>,

    /// Optional bind address for the MCP server.
    #[serde(default = "defaults::default_mcp_bind_address_opt")]
    pub bind_address: Option<String>,
}

impl Default for McpEndpointConfig {
    fn default() -> Self {
        Self {
            cors: defaults::default_cors(),
            bind_address: defaults::default_mcp_bind_address_opt(),
        }
    }
}

/// Authentication options for pulling artifacts from OCI registries.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum OciAuthentication {
    /// Basic auth with username and password.
    Basic { username: String, password: String },
    /// Bearer token auth.
    Bearer { token: String },
    /// Anonymous auth (no credentials).
    #[default]
    Anonymous,
}

/// Conversion from OciAuthentication to RegistryAuth.
///
/// Maps the configuration enum to the OCI client's authentication type.
impl From<OciAuthentication> for RegistryAuth {
    fn from(a: OciAuthentication) -> Self {
        match a {
            OciAuthentication::Anonymous => RegistryAuth::Anonymous,
            OciAuthentication::Basic { username, password } => {
                RegistryAuth::Basic(username, password)
            }
            OciAuthentication::Bearer { token } => RegistryAuth::Bearer(token),
        }
    }
}

// ----------------- Authentication (External Identity) -----------------

/// Configuration for a single external identity provider (Microsoft / Google).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityProviderConfig {
    /// Logical name referenced by `auth.provider` (e.g. "microsoft", "google").
    pub name: String,
    /// OAuth / OIDC client id.
    pub client_id: String,
    /// Optional client secret (only for confidential flows; not required for pure bearer validation).
    #[serde(default)]
    pub client_secret: Option<String>,
    /// Issuer / authority base URL.
    pub authority: String,
    /// Optional space-separated scopes (defaults to "openid profile email").
    #[serde(default)]
    pub scopes: Option<String>,
    /// Optional audience override (defaults to client_id if not set).
    #[serde(default)]
    pub audience: Option<String>,
    /// Whether to attempt OIDC discovery for endpoints and JWKS (default true).
    #[serde(default = "defaults::default_true")]
    pub discovery: bool,
    /// Optional explicit JWKS URI (overrides discovery & builtin heuristics).
    #[serde(default)]
    pub jwks_uri: Option<String>,
    /// Optional explicit authorization endpoint (overrides discovery if set).
    #[serde(default)]
    pub authorization_endpoint: Option<String>,
    /// Optional explicit token endpoint (overrides discovery if set).
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// Optional redirect_uri to include in authorization and token requests.
    #[serde(default)]
    pub redirect_uri: Option<String>,
    /// Optional additional scopes to request during authentication flows.
    #[serde(default)]
    pub additional_scopes: Option<Vec<String>>,
}

/// Session cookie configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default = "defaults::default_session_timeout")]
    pub timeout_seconds: u64,
    #[serde(default = "defaults::default_session_cookie_name")]
    pub cookie_name: String,
    #[serde(default = "defaults::default_true")]
    pub cookie_secure: bool,
    #[serde(default = "defaults::default_true")]
    pub cookie_http_only: bool,
    #[serde(default = "defaults::default_cookie_same_site")]
    pub same_site: String,
    /// Optional cookie domain (e.g., "localhost" or ".example.com")
    #[serde(default)]
    pub cookie_domain: Option<String>,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: defaults::default_session_timeout(),
            cookie_name: defaults::default_session_cookie_name(),
            cookie_secure: defaults::default_true(),
            cookie_http_only: defaults::default_true(),
            same_site: defaults::default_cookie_same_site(),
            cookie_domain: None,
        }
    }
}

/// Top-level authentication configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AuthConfig {
    #[serde(default = "defaults::default_false")]
    pub enabled: bool,
    /// Active provider name.
    #[serde(default)]
    pub provider: Option<String>,
    /// Declared provider configurations.
    #[serde(default)]
    pub providers: Vec<IdentityProviderConfig>,
    /// Optional session configuration (enables cookie-based auth for browser clients).
    #[serde(default)]
    pub session: Option<SessionConfig>,
}
