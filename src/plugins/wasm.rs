/// WASM plugin loader and executor using Extism.
///
/// This module provides functionality to load and execute WebAssembly (WASM) plugins
/// using the Extism runtime. It wraps an Extism `Plugin` in a thread-safe manner
/// and implements the `UriHandler` trait to load plugins from various sources.
///
/// The handler supports:
/// - Loading WASM modules from byte data
/// - Merging plugin manifests with Extism configurations
/// - Describing available tools via the plugin's `describe` export
/// - Executing tool calls via the plugin's `call` export
use crate::config::plugins::PluginManifest;
use crate::state::{DynExecFuture, ToolExecFn};
use crate::{config::plugins::ArkPlugin, plugins::ToolSet};
use anyhow::anyhow;
use extism::{Manifest, Plugin, Wasm};
use rmcp::ErrorData;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::debug;

/// Timeouts for WASM plugin operations (in seconds)
const DESCRIBE_TIMEOUT_SECS: u64 = 30;
const CALL_TIMEOUT_SECS: u64 = 120;

use super::UriHandler;

/// Handler for loading and executing WASM plugins using Extism.
///
/// This struct manages a single WASM plugin instance, providing methods to
/// describe available tools and build executors for tool invocations.
/// The plugin is wrapped in an `Arc<Mutex<Plugin>>` to allow safe concurrent
/// access from multiple async tasks.
pub struct WasmHandler {
    /// Shared plugin instance protected by a mutex for thread safety.
    plugin: Arc<Mutex<Plugin>>,
}

impl WasmHandler {
    /// Merges a base manifest with plugin configuration.
    ///
    /// # Arguments
    /// * `base` - The base Extism manifest
    /// * `plugin_cfg` - Optional plugin configuration to merge
    ///
    /// # Returns
    /// The merged manifest with configuration applied
    fn merge_manifest(mut base: Manifest, plugin_cfg: &Option<PluginManifest>) -> Manifest {
        if let Some(cfg) = plugin_cfg {
            if let Some(max_pages) = cfg.memory.as_ref().and_then(|memory| memory.max_pages) {
                base.memory.max_pages = Some(max_pages);
            }
            if let Some(config) = &cfg.config {
                base.config = config.clone();
            }
            if let Some(hosts) = &cfg.allowed_hosts {
                base.allowed_hosts = Some(hosts.clone());
            }
            if let Some(paths) = &cfg.allowed_paths {
                base.allowed_paths = Some(paths.clone());
            }
        }
        base
    }

    /// Constructs a new `WasmHandler` from raw WASM bytes.
    ///
    /// # Arguments
    /// * `bytes` - The raw WASM module data
    /// * `plugin_cfg` - Optional plugin manifest for configuration
    ///
    /// # Returns
    /// A `Result` containing the `WasmHandler` or an error if loading fails.
    ///
    /// # Details
    /// This method creates an Extism `Manifest` from the WASM data and merges
    /// any provided plugin configuration (memory limits, allowed hosts/paths, etc.).
    /// The plugin is instantiated with the merged manifest.
    pub fn new(bytes: Vec<u8>, plugin_cfg: &Option<PluginManifest>) -> anyhow::Result<Self> {
        let wasm = Wasm::data(bytes);
        let manifest = Manifest::new([wasm]);
        let merged = Self::merge_manifest(manifest, plugin_cfg);

        let plugin = Plugin::new(&merged, [], true)
            .map_err(|e| anyhow!("Failed to load WASM plugin: {e}"))?;

        Ok(Self {
            plugin: Arc::new(Mutex::new(plugin)),
        })
    }
}

impl WasmHandler {
    /// Describes the tools available in the WASM plugin.
    ///
    /// Calls the plugin's `describe` export function and parses the returned
    /// JSON into a `ToolSet` containing tool definitions.
    ///
    /// # Arguments
    /// * `config` - The plugin configuration (used for logging)
    ///
    /// # Returns
    /// A `Result` containing the `ToolSet` or an error if description fails.
    ///
    /// # Details
    /// This method runs the plugin's `describe` function in a blocking task
    /// with a 30-second timeout. The result is deserialized from JSON and
    /// logged for debugging purposes.
    pub async fn describe(&self, config: &ArkPlugin) -> anyhow::Result<ToolSet> {
        let plugin = Arc::clone(&self.plugin);
        let uri_owned = config
            .url
            .as_ref()
            .ok_or_else(|| anyhow!("Plugin config missing URL"))?
            .to_owned();

        let handle = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
            let mut plugin = plugin
                .lock()
                .map_err(|e| anyhow!("Failed to lock plugin: {e}"))?;
            plugin
                .call::<&str, String>("describe", "")
                .map_err(|e| anyhow!("WASM plugin describe() failed: {e}"))
        });

        let joined = tokio::time::timeout(Duration::from_secs(DESCRIBE_TIMEOUT_SECS), handle)
            .await
            .map_err(|_| {
                anyhow!(
                    "WASM plugin describe() timed out after {}s",
                    DESCRIBE_TIMEOUT_SECS
                )
            })?;
        let join_ok = joined.map_err(|e| anyhow!("WASM plugin describe() join error: {e}"))?;
        let json = join_ok?;

        let deserialized = serde_json::from_str::<ToolSet>(&json)?;

        debug!(
            "WASM plugin [{}] describe returned {} tools",
            super::sanitized_url(&uri_owned),
            deserialized.tools.len()
        );
        for tool in &deserialized.tools {
            debug!(
                "  - {}: {}",
                tool.name.clone(),
                tool.description.clone().unwrap_or("unknown".into())
            );
        }

        Ok(deserialized)
    }

    /// Builds an executor function for a specific tool.
    ///
    /// Returns a closure that can be used to invoke the specified tool on the plugin.
    /// The executor handles the MCP protocol conversion and async execution.
    ///
    /// # Arguments
    /// * `tool_name` - The name of the tool to execute
    ///
    /// # Returns
    /// A `ToolExecFn` closure that takes arguments and returns a future resolving to the result.
    ///
    /// # Details
    /// The executor wraps the tool arguments in the expected MCP `CallToolRequest` format
    /// (`{"params": {"name": tool_name, "arguments": args}}`), serializes it to JSON,
    /// and calls the plugin's `call` export. The response is parsed back to a `Value`.
    ///
    /// Execution is performed in a blocking task with a 120-second timeout to prevent
    /// hanging on long-running plugin operations. The `spawn_blocking` is used because
    /// WASM execution is CPU-bound and should not block the async runtime.
    pub fn build_executor(&self, tool_name: &str) -> ToolExecFn {
        let plugin = Arc::clone(&self.plugin);
        let tool_name = tool_name.to_string();
        Arc::new(move |args: Value| -> DynExecFuture {
            let plugin = Arc::clone(&plugin);
            let tool_name = tool_name.clone();
            Box::pin(async move {
                // Wrap args in "params" to match MCP CallToolRequest structure
                let input = json!({"params": {"name": tool_name, "arguments": args}});
                let input_str = serde_json::to_string(&input)
                    .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
                tracing::debug!("Sending to WASM plugin: {}", input_str);
                let handle = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                    let mut plugin = plugin
                        .lock()
                        .map_err(|e| anyhow!("Failed to lock plugin: {e}"))?;
                    plugin
                        .call::<&str, String>("call", &input_str)
                        .map_err(|e| anyhow!("WASM plugin call() failed: {e}"))
                });
                let joined = tokio::time::timeout(Duration::from_secs(CALL_TIMEOUT_SECS), handle)
                    .await
                    .map_err(|_| {
                        ErrorData::new(
                            rmcp::model::ErrorCode::INTERNAL_ERROR,
                            format!("WASM plugin call() timed out after {}s", CALL_TIMEOUT_SECS),
                            None,
                        )
                    })?;
                let join_ok = joined.map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                let json_text =
                    join_ok.map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
                let value: Value = serde_json::from_str(&json_text)
                    .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;
                Ok(value)
            })
        })
    }
}

impl UriHandler for WasmHandler {
    /// Loads the plugin and returns its tool set and executors.
    ///
    /// Implements the `UriHandler::get` method by calling `describe` to get
    /// the available tools, then building executors for each tool.
    ///
    /// # Arguments
    /// * `config` - The plugin configuration
    ///
    /// # Returns
    /// A `Result` containing `PluginLoadResult` with tools and executors, or an error.
    ///
    /// # Details
    /// This method is called during plugin loading to discover and prepare
    /// all tools provided by the WASM plugin.
    async fn get(&self, config: &ArkPlugin) -> anyhow::Result<super::PluginLoadResult> {
        let toolset = self.describe(config).await?;
        let mut execs = Vec::new();
        for t in &toolset.tools {
            let name = t.name.as_ref();
            let exec = self.build_executor(name);
            execs.push((name.to_string(), exec));
        }
        Ok(super::PluginLoadResult {
            toolset,
            executors: execs,
            raw_bytes: None,
            source_url: None,
        })
    }
}
