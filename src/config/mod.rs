pub use components::McpTransport;
use serde::{Deserialize, Serialize};
/**
 * Ark configuration root module.
 *
 * - Defines the root configuration struct (`ArkConfig`) and its defaults.
 * - Provides config file loading, CLI/env override logic, and error reporting.
 * - Uses `components.rs` for types/enums and `defaults.rs` for default helpers.
 */
use std::{path::Path, path::PathBuf, sync::Arc};
use thiserror::Error;

use components::{ManagementEndpointConfig, McpEndpointConfig};
use plugins::ArkPlugin;

use crate::config::components::TlsConfig;
use crate::state::ArkState;

pub mod components;
pub mod defaults;
pub mod plugins;

// Root configuration for the Ark server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArkConfig {
    /// MCP transport selection (stdio, sse, streamable-http).
    #[serde(default = "defaults::default_transport")]
    pub transport: Option<McpTransport>,

    /// Skip OCI signature verification (NOT recommended for production).
    #[serde(default = "defaults::default_false")]
    pub insecure_skip_signature: bool,

    /// Use Sigstore TUF data for verification (recommended default: true).
    #[serde(default = "defaults::default_true")]
    pub use_sigstore_tuf_data: bool,

    /// Expected certificate issuer for verification.
    #[serde(default)]
    pub cert_issuer: Option<String>,

    /// Expected certificate email for verification.
    #[serde(default)]
    pub cert_email: Option<String>,

    /// Expected certificate URL for verification.
    #[serde(default)]
    pub cert_url: Option<String>,

    /// Management server configuration.
    pub management_server: Option<ManagementEndpointConfig>,

    /// MCP server configuration.
    pub mcp_server: Option<McpEndpointConfig>,

    /// List of plugins to load at startup.
    #[serde(default)]
    pub plugins: Vec<ArkPlugin>,

    /// TLS configuration.
    #[serde(default)]
    pub tls: Option<TlsConfig>,

    /// Authentication configuration (optional)
    #[serde(default)]
    pub auth: Option<components::AuthConfig>,
}

impl ArkConfig {
    /// Compute the default configuration file path.
    pub fn default_path() -> PathBuf {
        // Allow override via environment variable
        if let Some(override_path) = std::env::var_os("ARK_CONFIG_PATH") {
            return PathBuf::from(override_path);
        }
        if cfg!(target_os = "windows") {
            // $HOME/ark.config
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .unwrap_or_default();
            let mut p = PathBuf::from(home);
            p.push("ark");
            p.push("config");
            p
        } else {
            // ~/.config/ark.config
            PathBuf::from("/").join("etc").join("ark").join("config")
        }
    }

    /// Create a default configuration when no file is present.
    ///
    /// This provides sensible defaults for all fields, allowing the application
    /// to run without a configuration file.
    fn default_config() -> Self {
        Self {
            transport: defaults::default_transport(),
            insecure_skip_signature: defaults::default_false(),
            use_sigstore_tuf_data: defaults::default_true(),
            cert_issuer: None,
            cert_email: None,
            cert_url: None,
            tls: None,
            management_server: Some(ManagementEndpointConfig::default()),
            mcp_server: Some(McpEndpointConfig::default()),
            plugins: Vec::new(),
            auth: None,
        }
    }

    /// Load config from file and apply CLI overrides.
    ///
    /// Loads configuration from a YAML file if it exists, otherwise uses defaults.
    /// Then applies command-line overrides with highest precedence.
    ///
    /// TODO: management_address override is not currently supported.
    ///
    /// # Arguments
    /// * `config_path` - Optional path to the configuration file. Uses default if None.
    /// * `transport` - MCP transport to use (overrides config file).
    /// * `mcp_bind_address` - Optional bind address for MCP server.
    /// * `insecure_skip_signature` - Whether to skip OCI signature verification.
    /// * `use_sigstore_tuf_data` - Whether to use Sigstore TUF data.
    /// * `disable_api` - Optional flag to disable plugin API.
    /// * `management_bind_address` - Optional bind address for management server.
    ///
    /// # Returns
    /// The loaded and overridden configuration, or a ConfigError.
    #[allow(clippy::too_many_arguments)]
    pub fn load_with_overrides(
        config_path: Option<PathBuf>,
        //log_level: Option<LogLevel>,
        transport: McpTransport,
        mcp_bind_address: Option<String>,
        insecure_skip_signature: bool,
        use_sigstore_tuf_data: bool,
        disable_api: Option<bool>,
        management_bind_address: Option<String>,
    ) -> Result<Self, ConfigError> {
        let path = config_path.unwrap_or_else(Self::default_path);

        // Parse from file with line/column + serde path diagnostics
        let mut cfg = if path.exists() {
            tracing::debug!("Reading from configuration file {:?}", path);
            let text = std::fs::read_to_string(&path)
                .map_err(|e| ConfigError::Parse(path.clone(), format!("I/O error: {}", e)))?;
            let parsed_cfg = Self::parse_yaml_with_path(&path, &text)?;

            // Ensure management_server and mcp_server have defaults even if missing from file
            Self {
                management_server: parsed_cfg
                    .management_server
                    .or_else(|| Some(ManagementEndpointConfig::default())),
                mcp_server: parsed_cfg
                    .mcp_server
                    .or_else(|| Some(McpEndpointConfig::default())),
                ..parsed_cfg
            }
        } else {
            tracing::warn!(
                "No configuration file (checked {:?}) initializing with defaults",
                path
            );
            // No file construct with defaults
            Self::default_config()
        };

        // Apply CLI/env overrides (highest precedence)
        //cfg.log_level = log_level;
        cfg.transport = Some(transport);
        cfg.insecure_skip_signature = insecure_skip_signature;
        cfg.use_sigstore_tuf_data = use_sigstore_tuf_data;

        if let (Some(addr), Some(ref mut mcp)) = (mcp_bind_address, cfg.mcp_server.as_mut()) {
            mcp.bind_address = Some(addr);
        }

        if let (Some(disabled), Some(ref mut mgmt)) = (disable_api, cfg.management_server.as_mut())
        {
            mgmt.disable_plugin_api = disabled;
        }
        if let (Some(addr), Some(ref mut mgmt)) =
            (management_bind_address, cfg.management_server.as_mut())
        {
            mgmt.bind_address = Some(addr);
        }

        Ok(cfg)
    }

    /// Parse YAML configuration with enhanced error reporting.
    ///
    /// Uses serde_yaml_ng to parse the YAML text, and includes line/column information
    /// in error messages for better debugging.
    ///
    /// # Arguments
    /// * `path` - Path to the configuration file (for error messages).
    /// * `text` - The YAML content as a string.
    ///
    /// # Returns
    /// The parsed configuration, or a ConfigError with detailed location info.
    fn parse_yaml_with_path(path: &Path, text: &str) -> Result<Self, ConfigError> {
        serde_yaml_ng::from_str::<Self>(text).map_err(|e| {
            let msg = if let Some(loc) = e.location() {
                format!(
                    "yaml error at line {}, column {}: {}",
                    loc.line(),
                    loc.column(),
                    e
                )
            } else {
                format!("yaml error: {}", e)
            };

            ConfigError::Parse(path.to_path_buf(), msg)
        })
    }
    /// Apply relevant config fields to the shared application state.
    ///
    /// Updates the application state with configuration values that affect runtime behavior,
    /// such as CORS settings, API disabling flags, and transport selection.
    ///
    /// # Arguments
    /// * `state` - The shared application state to update.
    pub async fn apply_to_state(&self, state: Arc<ArkState>) {
        let mgmt_srv = self.management_server.clone().unwrap_or_default();
        let _mcp_srv = self.mcp_server.clone().unwrap_or_default();

        let use_json = mgmt_srv.response_type.eq_ignore_ascii_case("json");
        state.set_use_json_management_responses(use_json);

        // Apply CORS settings to AppState for centralized HTTP handling

        state.set_disable_health_api(mgmt_srv.disable_health_api);
        state.set_disable_console(mgmt_srv.disable_console);
        state.set_disable_plugins_api(mgmt_srv.disable_plugin_api);
        state.set_disable_prometheus_api(mgmt_srv.disable_prometheus_api);
        state.set_transport(self.transport.unwrap_or_default());

        // Log auth summary (do not fail if misconfigured)
        if let Some(auth) = &self.auth {
            if auth.enabled {
                if let Some(active) = &auth.provider {
                    tracing::info!(
                        target = "ark.auth",
                        "authentication enabled (provider={})",
                        active
                    );
                } else {
                    tracing::warn!(
                        target = "ark.auth",
                        "authentication enabled but no active provider selected"
                    );
                }
            } else {
                tracing::warn!(target = "ark.auth", "authentication disabled");
            }
        } else {
            tracing::warn!(target = "ark.auth", "authentication not configured");
        }
    }
}

// Errors during configuration loading/parsing.
#[derive(Debug, Error)]
pub enum ConfigError {
    /// Failed to parse configuration content.
    ///
    /// Tuple fields:
    /// - 0: Path to the configuration file that failed to parse
    /// - 1: Error message from the underlying parser
    #[error("Failed to parse {0}: {1}")]
    Parse(PathBuf, String),
}
