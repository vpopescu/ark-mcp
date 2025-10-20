#![allow(dead_code)]
/// The application state is responsible for:
///
/// - Maintaining the state of the server
/// - Hosting the plugin registry
use crate::{
    config::models::McpTransport,
    config::plugins::ArkPlugin,
    plugins::{ToolSet, registry::PluginRegistry},
    server::auth::AuthState,
    server::persist::Database,
};
use anyhow::Result;
use rmcp::{
    ErrorData,
    model::{ServerInfo, Tool},
};

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};

use tracing::debug;

/** Application lifecycle states. */
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ApplicationState {
    /// Unknown state, typically the initial state.
    Unknown = 0,
    /// The application is initializing.
    Initializing = 1,
    /// Loading plugins.
    LoadingPlugins = 2,
    /// Starting network services.
    StartingNetwork = 3,
    /// The application is ready to serve requests.
    Ready = 4,
    /// The application is terminating.
    Terminating = 5,
}

// Shared application state and registry of plugins.
//
// This struct holds the core state of the Ark MCP server, including
// server information, lifecycle state, configuration flags, and the
// plugin registry that manages all loaded plugins and tools.
#[derive(Debug)]
pub struct ArkState {
    /// Server information for MCP protocol.
    pub server_info: Arc<ServerInfo>,
    /// Whether to use JSON responses for management endpoints.
    pub use_json_management_responses: AtomicBool,
    /// Current application lifecycle state.
    pub state: AtomicU8,
    /// Whether the health API is disabled.
    pub disable_health_api: AtomicBool,
    /// Whether the plugin management API is disabled.
    pub disable_plugin_api: AtomicBool,
    /// Whether the Prometheus metrics API is disabled.
    pub disable_prometheus_api: AtomicBool,
    /// Whether the admin console is disabled.
    pub disable_console: AtomicBool,
    /// Selected MCP transport (stdio, sse, streamablehttp).
    pub transport: RwLock<McpTransport>,
    /// Registry of all loaded plugins and their tools.
    pub plugin_registry: PluginRegistry,
    /// Authentication state (optional for testing).
    pub auth_state: RwLock<Option<Arc<AuthState>>>,
    /// Database for persistent storage (optional for testing).
    pub database: RwLock<Option<Database>>,
}

/// Default implementation for ArkState.
///
/// Initializes the state with default values, including an empty plugin registry
/// and default transport (Stdio). This used to have a manually derived Clone but
/// it's been removed since it's not needed today. May have to be added back
/// in the future.
impl Default for ArkState {
    fn default() -> Self {
        Self {
            use_json_management_responses: AtomicBool::new(false),
            state: AtomicU8::new(ApplicationState::Unknown as u8),
            disable_plugin_api: AtomicBool::new(false),
            disable_prometheus_api: AtomicBool::new(false),
            disable_console: AtomicBool::new(false),
            disable_health_api: AtomicBool::new(false),
            transport: RwLock::new(McpTransport::Stdio),
            plugin_registry: PluginRegistry::new_local(),
            server_info: Arc::new(ServerInfo::default()),
            auth_state: RwLock::new(None),
            database: RwLock::new(None),
        }
    }
}

/// Implementation of ArkState methods.
///
/// Provides methods for managing application state, configuration,
/// plugin registration, and tool execution.
// ArkState impl contains setters and helpers that are used by integration
// tests (tests/*). The editor/static analyzer may not see those external
// usages and will sometimes warn about dead code. Allow dead_code at the
// impl level with a clear comment so the code remains quiet in the IDE
// while preserving the public API for tests.
#[allow(dead_code)]
impl ArkState {
    /// Set application lifecycle state.
    pub fn set_state(&self, value: ApplicationState) {
        let v = value as u8;
        debug!("Application state changed to {:?}", v);
        self.state.store(v, Ordering::Relaxed);
    }

    /// Enable or disable JSON management responses.
    pub fn set_use_json_management_responses(&self, value: bool) {
        self.use_json_management_responses
            .store(value, Ordering::Relaxed);
    }

    /// Returns true if the application is running (liveness check).
    /// This is a basic check that the process is alive and not terminated.
    pub fn is_alive(&self) -> bool {
        let state = self.state.load(Ordering::SeqCst);
        state >= ApplicationState::Initializing as u8 && state < ApplicationState::Terminating as u8
    }

    /// Returns true if the application is ready to serve requests.
    /// This indicates the app has completed initialization and is fully operational.
    pub fn is_ready(&self) -> bool {
        self.state.load(Ordering::SeqCst) >= ApplicationState::Ready as u8
    }

    /// Returns true if the token signer (if required) is ready.
    ///
    /// Logic:
    /// - If there is no auth_state configured, signer readiness is true.
    /// - If auth_state exists and auth is disabled, signer readiness is true.
    /// - If auth_state exists and enabled, signer readiness is true only if signer is present.
    pub fn is_signer_ready(&self) -> bool {
        if let Ok(auth_opt) = self.auth_state.read()
            && let Some(auth) = auth_opt.as_ref()
        {
            // If auth is disabled, signer not required
            if !auth.enabled {
                return true;
            }
            return auth.signer.is_some();
        }
        // No auth configured => signer not required
        true
    }

    /// Enable/disable built-in HTTP API.
    pub fn set_disable_plugins_api(&self, value: bool) {
        debug!(
            "Plugins API server is {}",
            if value { "disabled" } else { "enabled" }
        );
        self.disable_plugin_api.store(value, Ordering::Relaxed);
    }

    /// Enable/disable health API.
    pub fn set_disable_health_api(&self, value: bool) {
        debug!(
            "Health API server is {}",
            if value { "disabled" } else { "enabled" }
        );
        self.disable_health_api.store(value, Ordering::Relaxed);
    }

    /// Enable/disable Prometheus metrics API.
    pub fn set_disable_prometheus_api(&self, value: bool) {
        debug!(
            "Prometheus API server is {}",
            if value { "disabled" } else { "enabled" }
        );
        self.disable_prometheus_api.store(value, Ordering::Relaxed);
    }

    /// Enable/disable admin console (static /admin).
    pub fn set_disable_console(&self, value: bool) {
        self.disable_console.store(value, Ordering::Relaxed);
    }

    /// Whether admin console is disabled.
    pub fn is_console_enabled(&self) -> bool {
        (match self.transport.read() {
            Ok(guard) => *guard != McpTransport::Stdio,
            Err(_) => McpTransport::default() != McpTransport::Stdio,
        }) && !self.disable_console.load(Ordering::Relaxed)
    }

    /// Whether health API is enabled.
    pub fn is_health_api_enabled(&self) -> bool {
        (match self.transport.read() {
            Ok(guard) => *guard != McpTransport::Stdio,
            Err(_) => McpTransport::default() != McpTransport::Stdio,
        }) && !self.disable_health_api.load(Ordering::Relaxed)
    }

    /// Whether plugin API is enabled.
    pub fn is_plugin_api_enabled(&self) -> bool {
        (match self.transport.read() {
            Ok(guard) => *guard != McpTransport::Stdio,
            Err(_) => McpTransport::default() != McpTransport::Stdio,
        }) && !self.disable_plugin_api.load(Ordering::Relaxed)
    }

    /// Returns true if the Prometheus metrics API is enabled.
    ///
    /// The API is enabled when the `prometheus` feature is compiled in
    /// and the `disable_prometheus_api` configuration is false.
    #[cfg(feature = "prometheus")]
    pub fn is_prometheus_api_enabled(&self) -> bool {
        (match self.transport.read() {
            Ok(guard) => *guard != McpTransport::Stdio,
            Err(_) => McpTransport::default() != McpTransport::Stdio,
        }) && !self.disable_prometheus_api.load(Ordering::Relaxed)
    }

    /// Returns true if the Prometheus metrics API is enabled.
    ///
    /// This is a no-op when the `prometheus` feature is not compiled in.
    #[cfg(not(feature = "prometheus"))]
    pub fn is_prometheus_api_enabled(&self) -> bool {
        false
    }

    /// Set selected transport (stdio, sse, streamablehttp).
    pub fn set_transport(&self, transport: McpTransport) {
        if let Ok(mut w) = self.transport.write() {
            *w = transport
        }
    }

    /// Set authentication state (for testing).
    /// This setter is intentionally an inherent method that accepts
    /// `self: &Arc<Self>` so it can be called directly on `Arc<ArkState>` from
    /// integration tests without requiring explicit deref coercions. This
    /// makes usages clearer to static analyzers and avoids editor false
    /// positives while keeping the API ergonomic for tests.
    pub fn set_auth_state(self: &Arc<Self>, auth_state: Arc<AuthState>) {
        let inner: &ArkState = Arc::as_ref(self);
        if let Ok(mut w) = inner.auth_state.write() {
            *w = Some(auth_state);
        }
    }

    /// Set database for persistent storage.
    pub fn set_database(&self, database: Database) {
        if let Ok(mut w) = self.database.write() {
            *w = Some(database);
        }
    }

    /// Get current transport.
    pub fn get_transport(&self) -> McpTransport {
        *self.transport.read().unwrap_or_else(|e| e.into_inner())
    }

    /// List tools, optionally filtered by plugin.
    ///
    /// # Arguments
    /// * `plugin_id` - Optional plugin ID to filter tools by
    ///
    /// # Returns
    /// A `Result` containing a vector of `Tool` definitions.
    pub async fn get_tools(&self, plugin_id: Option<&str>) -> Result<Vec<Tool>> {
        let tools = self.plugin_registry.tools(plugin_id).await?;
        Ok(tools)
    }

    /// Unregister a plugin and all its associated tools.
    ///
    /// Removes the plugin configuration, execution handlers, and all tools
    /// associated with the plugin from the registry.
    ///
    /// # Arguments
    /// * `plugin_name` - The name of the plugin to unregister
    ///
    /// # Returns
    /// `true` if the plugin was found and removed, `false` otherwise.
    pub async fn unregister_plugin(&self, plugin_name: &str) -> anyhow::Result<bool> {
        let mut catalog = self.plugin_registry.catalog.write().await;

        if !catalog.plugin_to_config.contains_key(plugin_name) {
            return Ok(false);
        }

        catalog.plugin_to_config.remove(plugin_name);
        catalog.tool_to_handler.remove(plugin_name);

        // Remove all tools associated with this plugin
        let tool_names: Vec<_> = catalog
            .tool_to_plugin
            .iter()
            .filter_map(|(tool_name, plugin)| {
                if plugin == plugin_name {
                    Some(tool_name.clone())
                } else {
                    None
                }
            })
            .collect();

        for tool_name in tool_names {
            catalog.tool_to_plugin.remove(&tool_name);
            catalog.tool_to_def.remove(&tool_name);
        }

        Ok(true)
    }

    /// Register a plugin with its tools and executors.
    ///
    /// This method registers a plugin's configuration, tool definitions,
    /// and execution handlers in the plugin registry. It establishes the
    /// mapping between tool names and their implementations.
    ///
    /// # Arguments
    /// * `plugin_config` - The plugin configuration
    /// * `toolset` - The set of tools provided by the plugin
    /// * `executors` - Vector of (tool_name, executor_function) pairs
    ///
    /// # Returns
    /// `Ok(())` on success, or an `ErrorData` if registration fails.
    ///
    /// # Details
    /// This method updates multiple internal maps:
    /// - `tool_to_plugin`: Maps tool names to plugin names
    /// - `tool_to_def`: Maps tool names to tool definitions
    /// - `plugin_to_config`: Stores plugin configurations
    /// - `tool_to_handler`: Maps tool names to execution functions
    pub async fn register_plugin_with_executors(
        &self,
        plugin_config: ArkPlugin,
        toolset: ToolSet,
        executors: Vec<(String, ToolExecFn)>,
    ) -> Result<(), rmcp::ErrorData> {
        let mut catalog = self.plugin_registry.catalog.write().await;
        // Ensure plugin has an owner; default to wildcard if none
        let mut plugin_config = plugin_config;
        if plugin_config.owner.is_none() {
            plugin_config.owner = Some("*/*/*".to_string());
        }

        // Register each tool's metadata
        for tool in &toolset.tools {
            let tool_name = tool.name.to_string();
            catalog
                .tool_to_plugin
                .insert(tool_name.clone(), plugin_config.name.clone());
            catalog.tool_to_def.insert(tool_name, tool.clone());
        }

        // Register plugin config
        catalog
            .plugin_to_config
            .insert(plugin_config.name.clone(), plugin_config.clone());

        // Register tool execution handlers
        for (tool_name, exec_fn) in executors {
            catalog.tool_to_handler.insert(tool_name, exec_fn);
        }

        Ok(())
    }
}

// Async function type used to execute a tool with JSON arguments and return JSON.
pub type DynExecFuture = Pin<Box<dyn Future<Output = Result<serde_json::Value, ErrorData>> + Send>>;
pub type ToolExecFn = Arc<dyn Fn(serde_json::Value) -> DynExecFuture + Send + Sync>;
