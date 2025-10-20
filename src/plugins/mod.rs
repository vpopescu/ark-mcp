//! Plugin loading and handler dispatch for Ark.
//!
//! This module provides the core plugin system for the Ark MCP server, including:
//! - Plugin loading from various sources (HTTP/HTTPS/file, OCI registries)
//! - Handler dispatch based on URL schemes
//! - Tool set definitions and execution
//! - Built-in plugin fallback when no external plugins are configured
//!
//! # Architecture
//!
//! The plugin system uses a URI-based handler pattern where different URL schemes
//! (http, https, file, oci) are routed to appropriate handlers that implement the
//! `UriHandler` trait. Each handler is responsible for fetching, validating, and
//! initializing plugins from their respective sources.
//!
//! # Plugin Loading Process
//!
//! 1. Configuration is parsed to identify plugin sources
//! 2. Each plugin URL is matched to a handler based on scheme
//! 3. Handler fetches and initializes the plugin
//! 4. Plugin's tool set is registered with execution handlers
//! 5. If no plugins are loaded, built-in diagnostic tools are registered

pub mod builtin;
pub mod oci;
pub mod registry;
pub mod url;
pub mod wasm;

// (JSON schema helper types were removed from the server; the frontend owns schema handling)
use std::sync::Arc;

use super::config::ArkConfig;
use crate::config::plugins::ArkPlugin;
use crate::plugins::builtin::{BUILTIN_PLUGIN_ID, BuiltinPlugin};
use crate::plugins::registry::{PluginHandler, ToolProvider};
use crate::state::{ArkState, ToolExecFn};
use ::url::Url;
use anyhow::{anyhow, bail};
use oci::OciHandler;
use rmcp::model::Tool;
use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use url::UrlHandler;

/// Result of loading a plugin from a URI source.
///
/// This struct encapsulates the complete result of plugin initialization,
/// including the tool definitions and their corresponding execution handlers.
pub struct PluginLoadResult {
    /// The tool set provided by the loaded plugin.
    pub toolset: ToolSet,
    /// Execution handlers for each tool, mapping tool names to functions.
    pub executors: Vec<(String, ToolExecFn)>,
    /// Optional raw payload bytes fetched during plugin load (e.g. WASM bytes).
    pub raw_bytes: Option<Vec<u8>>,
    /// Optional source URL string where the plugin was loaded from.
    pub source_url: Option<String>,
}

/// Trait for handling plugin loading from different URI schemes.
///
/// Implementors of this trait provide the logic for fetching and initializing
/// plugins from specific sources (HTTP, OCI registries, local files, etc.).
/// The trait normalizes the interface across different plugin sources.
pub trait UriHandler {
    /// Retrieve and initialize a plugin from the configured source.
    ///
    /// # Arguments
    /// * `plugin_config` - Configuration for the plugin to load
    ///
    /// # Returns
    /// A result containing the loaded plugin's tool set and executors, or an error.
    #[allow(async_fn_in_trait)]
    async fn get(&self, plugin_config: &ArkPlugin) -> anyhow::Result<PluginLoadResult>;
}

/// Content block within a tool execution result.
///
/// Represents a single piece of content returned by a tool execution,
/// typically containing text output with a specific type classification.
#[derive(Serialize)]
pub struct ToolContentBlock<'a> {
    /// The type of content (e.g., "text", "json", "error").
    #[serde(rename = "type")]
    pub kind: &'a str,
    /// The actual content text.
    pub text: &'a str,
}

/// Result of executing a tool.
///
/// Encapsulates the complete output of a tool execution, including
/// structured content blocks and error status.
#[derive(Serialize)]
pub struct ToolResult<'a> {
    /// Content blocks comprising the tool output.
    pub content: Vec<ToolContentBlock<'a>>,
    /// Structured representation of the result.
    #[serde(rename = "structuredContent")]
    pub structured_content: &'a str,
    /// Whether this result represents an error condition.
    #[serde(rename = "isError")]
    pub is_error: bool,
}

/// Builds execution handlers for all tools in a toolset.
///
/// Creates a mapping of tool names to execution functions that delegate
/// to the provided plugin handler. Each tool gets its own closure that
/// captures the necessary context for execution.
///
/// # Arguments
/// * `toolset` - The tool set containing tool definitions
/// * `handler` - The plugin handler that will execute the tools
///
/// # Returns
/// A vector of (tool_name, executor_function) pairs.
pub async fn build_executors(
    toolset: &ToolSet,
    handler: PluginHandler,
) -> Vec<(String, ToolExecFn)> {
    toolset
        .tools
        .iter()
        .map(|tool| {
            let name = tool.name.to_string();
            let handler = Arc::clone(&handler);

            let exec_fn: ToolExecFn = Arc::new(move |input: Value| {
                let handler = Arc::clone(&handler);
                handler(input)
            });

            (name, exec_fn)
        })
        .collect()
}

/// Loads all configured plugins and registers them with the application state.
///
/// This function iterates through the plugin configuration, loads each plugin
/// using the appropriate URI handler, and registers the resulting tools with
/// the plugin registry. If no plugins are successfully loaded, it falls back
/// to registering built-in diagnostic tools.
///
/// # Arguments
/// * `config` - Application configuration containing plugin definitions
/// * `state` - Application state for registering loaded plugins
///
/// # Returns
/// `Ok(())` if all plugins loaded successfully, or an error if loading failed.
///
/// # Behavior
/// - Loads plugins in the order they appear in configuration
/// - Falls back to built-in echo tool if no external plugins are loaded
/// - Logs progress and any failures during loading
pub async fn load_plugins(config: &ArkConfig, state: Arc<ArkState>) -> anyhow::Result<()> {
    tracing::debug!("Searching for configured plugins");

    for plugin in &config.plugins {
        let result = read_plugin_data(plugin)
            .await
            .map_err(|e| anyhow!("Failed to load plugin '{}': {}", plugin.name, e))?;

        let toolset = result.toolset;
        let executors = result.executors;

        state
            .register_plugin_with_executors(plugin.clone(), toolset, executors)
            .await?;
    }

    // If a database is configured, attempt to load any plugins persisted there
    if let Some(db) = state.database.read().ok().and_then(|g| g.clone()) {
        match db.list_plugins_async().await {
            Ok(records) => {
                tracing::debug!("Found {} persisted plugins in database", records.len());
                for rec in records {
                    // Skip plugins already present in the current config (by name)
                    if state
                        .plugin_registry
                        .catalog
                        .read()
                        .await
                        .plugin_to_config
                        .contains_key(&rec.plugin_id)
                    {
                        tracing::debug!(
                            "Skipping persisted plugin '{}' because it's already configured",
                            rec.plugin_id
                        );
                        continue;
                    }

                    // Try to load from raw bytes first (preferred)
                    if let Some(bytes) = rec.plugin_data.clone() {
                        tracing::debug!(
                            "Loading persisted plugin '{}' from stored bytes",
                            rec.plugin_id
                        );
                        // Attempt to reconstruct a minimal ArkPlugin for describe() logging
                        let plugin_cfg = ArkPlugin {
                            name: rec.plugin_id.clone(),
                            url: rec.plugin_path.as_ref().and_then(|s| Url::parse(s).ok()),
                            auth: None,
                            insecure: false,
                            manifest: serde_json::from_value::<
                                crate::config::plugins::PluginManifest,
                            >(
                                rec.metadata
                                    .get("manifest")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            )
                            .ok(),
                            owner: Some(rec.owner.clone()),
                        };
                        match wasm::WasmHandler::new(bytes, &plugin_cfg.manifest) {
                            Ok(wasm) => match wasm.describe(&plugin_cfg).await {
                                Ok(toolset) => {
                                    let executors = toolset
                                        .tools
                                        .iter()
                                        .map(|t| {
                                            (
                                                t.name.to_string(),
                                                wasm.build_executor(t.name.as_ref()),
                                            )
                                        })
                                        .collect();
                                    let mut plugin_cfg_clone = plugin_cfg.clone();
                                    plugin_cfg_clone.owner = Some(rec.owner.clone());
                                    if let Err(e) = state
                                        .register_plugin_with_executors(
                                            plugin_cfg_clone,
                                            toolset,
                                            executors,
                                        )
                                        .await
                                    {
                                        tracing::warn!(
                                            "Failed to register persisted plugin '{}' from DB: {:?}",
                                            rec.plugin_id,
                                            e
                                        );
                                    }
                                }
                                Err(e) => tracing::warn!(
                                    "Failed to describe persisted plugin '{}' from DB: {:?}",
                                    rec.plugin_id,
                                    e
                                ),
                            },
                            Err(e) => tracing::warn!(
                                "Failed to initialize Wasm for persisted plugin '{}' from DB: {:?}",
                                rec.plugin_id,
                                e
                            ),
                        }
                        continue;
                    }

                    // If no bytes stored but we have a path, try to load from that path
                    if let Some(path_str) = rec.plugin_path.as_ref()
                        && let Ok(url) = Url::parse(path_str)
                    {
                        tracing::debug!(
                            "Attempting to load persisted plugin '{}' from path {}",
                            rec.plugin_id,
                            url
                        );
                        let reconstructed = ArkPlugin {
                            name: rec.plugin_id.clone(),
                            url: Some(url),
                            auth: None,
                            insecure: false,
                            manifest: serde_json::from_value(
                                rec.metadata
                                    .get("manifest")
                                    .cloned()
                                    .unwrap_or(serde_json::Value::Null),
                            )
                            .ok(),
                            owner: Some(rec.owner.clone()),
                        };
                        match read_plugin_data(&reconstructed).await {
                            Ok(result) => {
                                if let Err(e) = state
                                    .register_plugin_with_executors(
                                        reconstructed,
                                        result.toolset,
                                        result.executors,
                                    )
                                    .await
                                {
                                    tracing::warn!(
                                        "Failed to register persisted plugin '{}' loaded from path: {:?}",
                                        rec.plugin_id,
                                        e
                                    );
                                }
                            }
                            Err(e) => tracing::warn!(
                                "Failed to reload persisted plugin '{}' from path: {:?}",
                                rec.plugin_id,
                                e
                            ),
                        }
                    }
                }
            }
            Err(e) => tracing::warn!("Failed to read persisted plugins from DB: {:?}", e),
        }
    }

    if state.plugin_registry.tools(None).await?.is_empty() {
        tracing::warn!("No plugins loaded, registering builtin diagnostic plugins");

        let plugin = Arc::new(BuiltinPlugin);
        let (plugin_config, toolset) = BuiltinPlugin::as_plugin_config(BUILTIN_PLUGIN_ID, &*plugin);

        let plugin_clone = Arc::clone(&plugin);
        let handler: PluginHandler = Arc::new(move |input: Value| {
            let plugin = Arc::clone(&plugin_clone);
            Box::pin(async move { plugin.call(&input).await })
        });

        let executors = build_executors(&toolset, handler).await;

        state
            .register_plugin_with_executors(plugin_config, toolset, executors)
            .await?;

        tracing::debug!("Builtin EchoPlugin registered");
    }

    Ok(())
}

/// Reads and loads plugin data from the configured source.
///
/// This function determines the appropriate handler based on the plugin's URL scheme
/// and delegates the loading process to that handler. Supported schemes include
/// HTTP/HTTPS/file and OCI registries.
///
/// # Arguments
/// * `plugin` - Plugin configuration containing URL and settings
///
/// # Returns
/// A result containing the loaded plugin data or an error.
///
/// # Supported Schemes
/// - `http` / `https` / `file` - Handled by `UrlHandler`
/// - `oci` - Handled by `OciHandler`
///
/// # Errors
/// Returns an error for unsupported URL schemes or loading failures.
pub async fn read_plugin_data(plugin: &ArkPlugin) -> anyhow::Result<PluginLoadResult> {
    tracing::debug!("Loading plugin with configuration {:?}", plugin);
    if plugin.clone().url.is_none() {
        bail!("Missing plugin path");
    }
    let url = plugin
        .url
        .clone()
        .ok_or_else(|| anyhow!("Missing plugin path"))?;

    let scheme = url.scheme();
    let result = match scheme {
        "http" | "https" | "file" => {
            let h = UrlHandler;
            h.get(plugin).await
        }
        "oci" => {
            let h = OciHandler;
            h.get(plugin).await
        }
        _ => {
            tracing::warn!("Scheme {} for path {} is not supported", scheme, url);
            bail!("Unsupported plugin scheme: {}", url.scheme());
        }
    }?;

    tracing::debug!(
        "Loaded plugin ToolSet [{}]: {} tools",
        result.toolset.name,
        result.toolset.tools.len()
    );

    Ok(result)
}

/// Top-level toolset descriptor provided by a plugin's `describe` export.
///
/// This struct represents the complete set of tools provided by a plugin,
/// as returned by the plugin's description interface. It supports flexible
/// JSON structures where tools can be under a "tools" key or any single
/// top-level key.
#[derive(Debug, Serialize, Clone)]
pub struct ToolSet {
    /// The top-level key name containing the tools array.
    pub name: String,
    /// Array of tool definitions provided by the plugin.
    pub tools: Vec<Tool>,
}

impl<'de> Deserialize<'de> for ToolSet {
    /// Custom deserialization supporting flexible toolset formats.
    ///
    /// Accepts JSON objects with either:
    /// - An explicit "tools" key containing the tool array
    /// - A single arbitrary key containing the tool array
    ///
    /// This flexibility allows plugins to use different naming conventions
    /// for their tool exports.
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        match value {
            serde_json::Value::Object(mut map) => {
                // Case 1: explicit "tools" key
                if let Some(v) = map.remove("tools") {
                    let tools: Vec<Tool> = serde_json::from_value(v).map_err(D::Error::custom)?;
                    return Ok(Self {
                        name: "tools".to_string(),
                        tools,
                    });
                }
                // Case 2: exactly one arbitrary key
                if map.len() == 1 {
                    let (name, v) = map.into_iter().next().unwrap();
                    let tools: Vec<Tool> = serde_json::from_value(v).map_err(D::Error::custom)?;
                    return Ok(Self { name, tools });
                }
                Err(D::Error::custom(
                    "expected object with 'tools' or a single top-level key",
                ))
            }
            _ => Err(D::Error::custom("expected a JSON object")),
        }
    }
}

// JSON schema helper types removed (frontend handles schema details)

/// Type alias for property definitions in input schemas.
/// Produces a sanitized URL string for logging purposes.
///
/// Removes sensitive information like credentials, query parameters,
/// and fragments from URLs before logging them. This prevents
/// accidental exposure of secrets in log files.
///
/// # Arguments
/// * `url` - The URL to sanitize
///
/// # Returns
/// A sanitized string representation of the URL
///
/// # Examples
/// - `file:///path/to/plugin.wasm` → `file:///path/to/plugin.wasm`
/// - `https://user:pass@example.com/plugin?token=secret` → `https://example.com/plugin`
pub fn sanitized_url(url: &Url) -> String {
    match url.scheme() {
        "file" => url.as_str().to_string(),
        _ => {
            let host = url.host_str().unwrap_or("");
            let port = url.port().map(|p| format!(":{}", p)).unwrap_or_default();
            format!("{}://{}{}{}", url.scheme(), host, port, url.path())
        }
    }
}
