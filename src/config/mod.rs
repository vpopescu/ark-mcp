pub use models::McpTransport;
#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use std::{path::Path, path::PathBuf, sync::Arc};
use thiserror::Error;

use models::{ManagementEndpointConfig, McpEndpointConfig};
use plugins::ArkPlugin;

use crate::config::models::TlsConfig;
use crate::state::ArkState;

pub mod defaults;
pub mod models;
pub mod plugins;

// Root configuration for the Ark server.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
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
    pub auth: Option<models::AuthConfig>,
    /// Optional token signing configuration (controls local signing/JWKS)
    #[serde(default)]
    pub token_signing: Option<models::TokenSigningConfig>,
}

impl ArkConfig {
    /// Compute the default configuration file path.
    pub fn default_path() -> PathBuf {
        // Allow override via environment variable
        if let Some(override_path) = std::env::var_os("ARK_CONFIG_PATH") {
            return PathBuf::from(override_path);
        }
        if cfg!(target_os = "windows") {
            // %HOME%/ark/config.yaml or %USERPROFILE%/ark/config.yaml
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .unwrap_or_default();
            let mut p = PathBuf::from(home);
            p.push("ark");
            p.push("config.yaml");
            p
        } else {
            // /etc/ark/config.yaml
            PathBuf::from("/")
                .join("etc")
                .join("ark")
                .join("config.yaml")
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
            token_signing: None,
        }
    }

    /// Load config from file and apply CLI overrides.
    ///
    /// Loads configuration from a YAML file if it exists, otherwise uses defaults.
    /// Then applies command-line overrides with highest precedence.
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
        let mut cfg = Self::load_config_from_file_or_defaults(&path)?;

        Self::apply_cli_overrides(
            &mut cfg,
            transport,
            mcp_bind_address,
            insecure_skip_signature,
            use_sigstore_tuf_data,
            disable_api,
            management_bind_address,
        );

        // Apply env overrides
        cfg.auth = Self::apply_auth_env_overrides(cfg.auth);
        cfg.tls = Self::apply_tls_env_overrides(cfg.tls);

        Ok(cfg)
    }

    /// Load config from file or use defaults.
    ///
    /// # Arguments
    /// * `path` - Path to check for config file
    ///
    /// # Returns
    /// Loaded config or defaults
    fn load_config_from_file_or_defaults(path: &Path) -> Result<Self, ConfigError> {
        if path.exists() {
            tracing::debug!("Reading from configuration file {:?}", path);
            let text = std::fs::read_to_string(path)
                .map_err(|e| ConfigError::Parse(path.to_path_buf(), format!("I/O error: {}", e)))?;
            let parsed_cfg = Self::parse_yaml_with_path(path, &text)?;

            // Ensure defaults for missing sections
            Ok(Self {
                management_server: parsed_cfg
                    .management_server
                    .or_else(|| Some(ManagementEndpointConfig::default())),
                mcp_server: parsed_cfg
                    .mcp_server
                    .or_else(|| Some(McpEndpointConfig::default())),
                ..parsed_cfg
            })
        } else {
            tracing::warn!("No configuration file (checked {:?}), using defaults", path);
            Ok(Self::default_config())
        }
    }

    /// Apply CLI overrides to config.
    ///
    /// # Arguments
    /// * `cfg` - Mutable config to update
    /// * `transport` - MCP transport override
    /// * `mcp_bind_address` - MCP bind address override
    /// * `insecure_skip_signature` - Signature skip flag
    /// * `use_sigstore_tuf_data` - Sigstore TUF flag
    /// * `disable_api` - API disable flag
    /// * `management_bind_address` - Management bind address override
    fn apply_cli_overrides(
        cfg: &mut Self,
        transport: McpTransport,
        mcp_bind_address: Option<String>,
        insecure_skip_signature: bool,
        use_sigstore_tuf_data: bool,
        disable_api: Option<bool>,
        management_bind_address: Option<String>,
    ) {
        cfg.transport = Some(transport);
        cfg.insecure_skip_signature = insecure_skip_signature;
        cfg.use_sigstore_tuf_data = use_sigstore_tuf_data;

        if let (Some(addr), Some(mcp)) = (mcp_bind_address, cfg.mcp_server.as_mut()) {
            mcp.bind_address = Some(addr);
        }
        if let (Some(disabled), Some(mgmt)) = (disable_api, cfg.management_server.as_mut()) {
            mgmt.disable_plugin_api = disabled;
        }
        if let (Some(addr), Some(mgmt)) = (management_bind_address, cfg.management_server.as_mut())
        {
            mgmt.bind_address = Some(addr);
        }
    }

    /// Apply authentication configuration from environment variables.
    ///
    /// This allows authentication to be configured entirely through environment variables
    /// without requiring a configuration file.
    fn apply_auth_env_overrides(
        existing_auth: Option<models::AuthConfig>,
    ) -> Option<models::AuthConfig> {
        use models::{AuthConfig, IdentityProviderConfig, SessionConfig};

        // Check if any auth env vars are set
        let auth_enabled = std::env::var("ARK_AUTH_ENABLED")
            .ok()
            .and_then(|v| v.parse::<bool>().ok());
        let auth_provider = std::env::var("ARK_AUTH_PROVIDER").ok();
        let auth_authority = std::env::var("ARK_AUTH_AUTHORITY").ok();
        let client_id = std::env::var("ARK_AUTH_CLIENT_ID").ok();
        let client_secret = std::env::var("ARK_AUTH_CLIENT_SECRET").ok();
        let auth_scopes = std::env::var("ARK_AUTH_SCOPES").ok();
        let groups_admin = std::env::var("ARK_AUTH_GROUPS_ADMIN").ok();
        let groups_users = std::env::var("ARK_AUTH_GROUPS_USERS").ok();

        // Entra ID specific vars
        let entra_tenant_id = std::env::var("ARK_AUTH_ENTRA_TENANT_ID").ok();

        // If no auth env vars are set, return existing config
        if auth_enabled.is_none()
            && auth_provider.is_none()
            && auth_authority.is_none()
            && client_id.is_none()
            && client_secret.is_none()
            && auth_scopes.is_none()
            && groups_admin.is_none()
            && groups_users.is_none()
            && entra_tenant_id.is_none()
        {
            return existing_auth;
        }

        // Start with existing config or create new one
        let mut auth_config = existing_auth.unwrap_or_else(|| AuthConfig {
            enabled: false,
            provider: None,
            providers: Vec::new(),
            session: Some(SessionConfig::default()),
        });

        // Apply environment variable overrides
        if let Some(enabled) = auth_enabled {
            auth_config.enabled = enabled;
        }
        if let Some(provider) = auth_provider {
            auth_config.provider = Some(provider.clone());

            // Build provider configs based on the selected provider
            let mut providers = Vec::new();

            if provider == "microsoft" || provider == "entra" {
                if let Some(cid) = client_id {
                    let authority: Option<String> = if let Some(auth) = &auth_authority {
                        Some(auth.clone())
                    } else if let Some(tenant_id) = entra_tenant_id {
                        Some(format!(
                            "https://login.microsoftonline.com/{}/v2.0",
                            tenant_id
                        ))
                    } else {
                        // If neither authority nor tenant_id provided, skip this provider
                        tracing::warn!(
                            "ARK_AUTH_AUTHORITY or ARK_AUTH_ENTRA_TENANT_ID must be provided for Microsoft/Entra provider"
                        );
                        None
                    };

                    if let Some(auth_url) = authority {
                        let scopes = auth_scopes
                            .clone()
                            .unwrap_or_else(|| "openid profile email".to_string());
                        let groups = if groups_admin.is_some() || groups_users.is_some() {
                            Some(models::Groups {
                                admin: groups_admin.clone(),
                                users: groups_users.clone(),
                            })
                        } else {
                            None
                        };
                        providers.push(IdentityProviderConfig {
                            name: "microsoft".to_string(),
                            client_id: cid,
                            client_secret: client_secret.clone(),
                            authority: auth_url,
                            scopes: Some(scopes),
                            audience: None,
                            discovery: true,
                            jwks_uri: None,
                            authorization_endpoint: None,
                            token_endpoint: None,
                            redirect_uri: None,
                            additional_scopes: None,
                            groups,
                        });
                    }
                }
            } else if provider == "google"
                && let Some(cid) = client_id
            {
                let scopes = auth_scopes
                    .clone()
                    .unwrap_or_else(|| "openid profile email".to_string());
                let groups = if groups_admin.is_some() || groups_users.is_some() {
                    Some(models::Groups {
                        admin: groups_admin.clone(),
                        users: groups_users.clone(),
                    })
                } else {
                    None
                };
                providers.push(IdentityProviderConfig {
                    name: "google".to_string(),
                    client_id: cid,
                    client_secret: client_secret.clone(),
                    authority: "https://accounts.google.com".to_string(),
                    scopes: Some(scopes),
                    audience: None,
                    discovery: true,
                    jwks_uri: None,
                    authorization_endpoint: None,
                    token_endpoint: None,
                    redirect_uri: None,
                    additional_scopes: None,
                    groups,
                });
            }

            if !providers.is_empty() {
                auth_config.providers = providers;
            }
        }

        Some(auth_config)
    }

    /// Apply TLS configuration from environment variables.
    ///
    /// This allows TLS to be configured entirely through environment variables
    /// without requiring a configuration file.
    fn apply_tls_env_overrides(
        existing_tls: Option<models::TlsConfig>,
    ) -> Option<models::TlsConfig> {
        use models::TlsConfig;

        // Check if any TLS env vars are set
        let tls_key = std::env::var("ARK_TLS_KEY").ok();
        let tls_cert = std::env::var("ARK_TLS_CERT").ok();
        let tls_silent_insecure = std::env::var("ARK_TLS_SILENT_INSECURE")
            .ok()
            .and_then(|v| v.parse::<bool>().ok());

        // If no TLS env vars are set, return existing config
        if tls_key.is_none() && tls_cert.is_none() && tls_silent_insecure.is_none() {
            return existing_tls;
        }

        // Start with existing config or create new one
        let mut tls_config = existing_tls.unwrap_or(TlsConfig {
            key: None,
            cert: None,
            silent_insecure: false,
        });

        // Apply environment variable overrides
        if let Some(key) = tls_key {
            tls_config.key = Some(key);
        }
        if let Some(cert) = tls_cert {
            tls_config.cert = Some(cert);
        }
        if let Some(silent_insecure) = tls_silent_insecure {
            tls_config.silent_insecure = silent_insecure;
        }

        Some(tls_config)
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
