//! Plugin configuration structures and utilities.
//!
//! This module defines the data structures used to configure plugins in the Ark MCP server,
//! including plugin manifests, authentication, and serialization helpers for flexible
//! plugin loading from various sources (files, URLs, OCI registries).

use crate::plugins::ToolSet;
use crate::plugins::registry::ToolProvider;
use crate::state::ToolExecFn;

use super::defaults;
use super::models::OciAuthentication;
use rmcp::ErrorData;
#[cfg(feature = "schemars")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use url::Url;

/// Plugin entry as configured by the user.
///
/// This struct represents a plugin configuration that can be loaded from various sources
/// including local files, remote URLs, and OCI registries. It supports flexible authentication
/// and security settings for different deployment scenarios.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct ArkPlugin {
    /// Friendly name for the plugin used for identification and logging.
    pub name: String,
    /// Location of the plugin (e.g., file:///..., http(s)://..., oci://...).
    /// Can be a file path or URL, automatically converted during deserialization.
    #[serde(
        rename = "url",
        alias = "path",
        deserialize_with = "deserialize_url_or_path"
    )]
    #[cfg_attr(feature = "schemars", schemars(with = "String"))]
    pub url: Option<Url>,
    /// Optional authentication used by handlers that need it (e.g., OCI registries).
    #[serde(rename = "config", alias = "options")]
    pub auth: Option<OciAuthentication>,
    /// Allow insecure transports (e.g., HTTP) for handlers that support it.
    /// Defaults to false for security.
    #[serde(default = "defaults::default_false")]
    pub insecure: bool,
    /// Optional plugin manifest containing runtime configuration.
    pub manifest: Option<PluginManifest>,
    /// Optional owner identity for the plugin in the canonical global id format
    /// (provider:tenant:userid). This can be used to record which authenticated
    /// Principal registered or owns the plugin.
    #[serde(default)]
    pub owner: Option<String>,
}

impl ArkPlugin {
    /// Creates a new plugin configuration without a path.
    ///
    /// Useful for plugins that are configured entirely through manifests
    /// or loaded through other mechanisms.
    ///
    /// # Arguments
    /// * `name` - Friendly name for the plugin
    /// * `manifest` - Optional plugin manifest
    ///
    /// # Returns
    /// A new `ArkPlugin` instance with default settings.
    pub fn new(name: String, manifest: Option<PluginManifest>) -> ArkPlugin {
        ArkPlugin {
            name,
            url: None,
            auth: None,
            insecure: false,
            manifest,
            owner: None,
        }
    }
}

/// Tool provider implementation for plugin tuples.
///
/// This implementation allows `(ArkPlugin, ToolSet, HashMap<String, ToolExecFn>)` tuples
/// to be used as tool providers in the plugin registry system. The tuple contains:
/// - The plugin configuration
/// - The tool set metadata
/// - A mapping of tool names to their execution functions
#[async_trait::async_trait]
impl ToolProvider for (ArkPlugin, ToolSet, HashMap<String, ToolExecFn>) {
    /// Returns the complete tool set provided by this plugin.
    fn toolset(&self) -> ToolSet {
        self.1.clone()
    }

    /// Executes a tool with the given input parameters.
    ///
    /// # Arguments
    /// * `input` - JSON value containing tool name and arguments
    ///
    /// # Returns
    /// A `Result` containing the tool execution result or an error.
    ///
    /// # Errors
    /// Returns an error if the requested tool is not found or execution fails.
    async fn call(&self, input: &Value) -> Result<Value, ErrorData> {
        let tool_name = input.get("tool").and_then(Value::as_str);
        let exec = match tool_name.and_then(|name| self.2.get(name)) {
            Some(exec) => exec.clone(),
            None => {
                return Err(ErrorData::method_not_found::<
                    rmcp::model::CallToolRequestMethod,
                >());
            }
        };

        exec(input.clone())
            .await
            .map_err(|e| ErrorData::internal_error(e.to_string(), None))
    }
}

/// Example plugin manifest structure:
/// ```json
/// {
///   "wasm": [
///     "./target/wasm32-wasi/release/my_plugin.wasm",
///     "https://example.com/deps/util.wasm",
///     "BASE64_WASM_BYTES=="
///   ],
///   "memory": { "max_pages": 1024 },
///   "config": {
///     "SERVICE_BASE_URL": "https://api.example.com",
///     "FEATURE_FLAG_X": "true",
///     "TENANT_ID": "contoso"
///   },
///   "allowed_hosts": [ "api.example.com", "telemetry.example.net" ],
///   "allowed_paths": {
///     "/var/log/extism": "/write/path",
///     "/etc/my_plugin": "/read/path",
///     "/tmp": "/readwrite/path"
///   }
/// }
/// ```
/// Top-level manifest structure for Extism plugins.
///
/// This structure defines the configuration for loading and running WebAssembly plugins
/// using the Extism runtime. All fields are optional to allow flexible plugin assembly
/// and deployment scenarios.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct PluginManifest {
    /// WASM module(s) to load — can be file paths, URLs, or base64-encoded data.
    /// Multiple modules allow for plugin composition and dependency management.
    pub wasm: Option<Vec<String>>,
    /// Memory and buffer limits for sandboxing controls.
    /// Restricts the plugin's memory usage for security and resource management.
    pub memory: Option<MemoryLimits>,
    /// Arbitrary plugin configuration passed at runtime.
    /// Key-value pairs that the plugin can access during execution.
    pub config: Option<BTreeMap<String, String>>,
    /// Allowed HTTP domains for network access restrictions.
    /// Limits the plugin's ability to make outbound HTTP requests for security.
    pub allowed_hosts: Option<Vec<String>>,
    /// Filesystem access mapping for WASI-enabled plugins.
    /// Maps host paths to plugin-accessible paths with read/write permissions.
    pub allowed_paths: Option<BTreeMap<String, PathBuf>>,
}

/// Memory allocation limits for the plugin.
///
/// Defines constraints on the WebAssembly plugin's memory usage to prevent
/// resource exhaustion and ensure sandboxing security.
#[derive(Debug, Serialize, Deserialize, Clone)]
#[cfg_attr(feature = "schemars", derive(JsonSchema))]
pub struct MemoryLimits {
    /// Maximum memory pages (1 page = 64KiB).
    /// Limits total memory allocation for the plugin instance.
    pub max_pages: Option<u32>,
}

/// Deserializes a string that could be either a URL or a file path.
///
/// This custom deserializer provides flexible input handling for plugin locations,
/// automatically converting file paths to `file://` URLs while preserving
/// existing URLs. Relative paths are resolved against the current working directory.
///
/// # Arguments
/// * `deserializer` - The serde deserializer
///
/// # Returns
/// An optional URL, or a deserialization error
///
/// # Errors
/// Returns an error if the input cannot be parsed as a URL or converted from a path.
fn deserialize_url_or_path<'de, D>(deserializer: D) -> Result<Option<Url>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;

    // First try to parse as a URL
    match Url::parse(&s) {
        Ok(url) => Ok(Some(url)),
        Err(_) => {
            // If parsing as URL fails, treat as a file path and convert to file:// URL
            let path = std::path::Path::new(&s);

            // Convert to absolute path if it's relative
            let absolute_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                // For relative paths, resolve against current working directory
                match std::env::current_dir() {
                    Ok(cwd) => cwd.join(path),
                    Err(e) => {
                        return Err(serde::de::Error::custom(format!(
                            "Failed to get current directory: {}",
                            e
                        )));
                    }
                }
            };

            // Convert path to URL and handle errors
            let url_string = file_path_to_url(&absolute_path).map_err(|e| {
                serde::de::Error::custom(format!("Failed to convert path to URL: {}", e))
            })?;

            // Parse the URL string to ensure validity
            match Url::parse(&url_string) {
                Ok(url) => Ok(Some(url)),
                Err(e) => Err(serde::de::Error::custom(format!(
                    "Failed to parse URL string from path: {}",
                    e
                ))),
            }
        }
    }
}

/// Converts a file system path to a `file://` URL.
///
/// # Arguments
/// * `path` - The file system path to convert
///
/// # Returns
/// A result containing the URL string or an error
///
/// # Errors
/// Returns an error if the path cannot be converted to a valid URL
/// (e.g., on Windows with non-UTF-8 paths).
pub fn file_path_to_url<P: AsRef<Path>>(path: P) -> anyhow::Result<String> {
    let path_str = path.as_ref().to_string_lossy();
    let url = url::Url::from_file_path(path_str.as_ref())
        .map_err(|e| anyhow::anyhow!("Failed to convert path '{}' to URL: {:?}", path_str, e))?;
    Ok(url.to_string())
}
