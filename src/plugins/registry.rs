use futures::future::BoxFuture;
use rmcp::{ErrorData, model::Tool, serde_json::Value};
use std::{collections::HashMap, fmt, sync::Arc};

use crate::config::plugins::ArkPlugin;
use crate::plugins::ToolSet;

/// Type alias for plugin executable handlers (async). Handlers receive an owned
/// Value to avoid borrow/lifetime issues crossing await points.
pub type PluginHandler =
    Arc<dyn Fn(Value) -> BoxFuture<'static, Result<Value, ErrorData>> + Send + Sync + 'static>;

/// Application inner store holding plugin metadata and handlers (moved inside PluginRegistry).
#[derive(Default)]
pub struct PluginStore {
    /// Plugin definitions mapped by plugin ID.
    pub plugin_to_config: HashMap<String, ArkPlugin>,

    /// Tool name to owner plugin ID mapping.
    pub tool_to_plugin: HashMap<String, String>,

    /// Tool name to handler mapping.
    pub tool_to_handler: HashMap<String, PluginHandler>,

    /// Tool name to tool definition mapping.
    pub tool_to_def: HashMap<String, Tool>,
}

impl PluginStore {
    /// Creates a new empty plugin store.
    pub fn new() -> Self {
        Self::default()
    }
}

/// A tool-provider trait for a plugin.
#[async_trait::async_trait]
pub trait ToolProvider {
    /// Returns the name of the tool provider.
    //fn name(&self) -> String;
    /// Returns the toolset provided by this plugin.
    fn toolset(&self) -> ToolSet;
    /// Calls the tool with the given input.
    async fn call(&self, input: &Value) -> Result<Value, ErrorData>;
}

/// Custom Debug implementation for PluginStore
impl fmt::Debug for PluginStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PluginStore")
            .field("plugins", &self.plugin_to_config.keys())
            .field(
                "handlers",
                &format_args!("<{} handlers>", self.tool_to_handler.len()),
            )
            .finish()
    }
}

/// Registry providing CRUD and invocation. It owns the PluginStore behind a Tokio RwLock.
#[derive(Clone, Debug)]
pub struct PluginRegistry {
    /// The underlying plugin store protected by a read-write lock.
    pub catalog: Arc<tokio::sync::RwLock<PluginStore>>,
}

impl PluginRegistry {
    /// Checks if a handler is registered for the given tool name.
    pub fn has_handler(&self, name: &str) -> bool {
        tokio::task::block_in_place(|| {
            self.catalog
                .blocking_read()
                .tool_to_handler
                .contains_key(name)
        })
    }

    /// Create a registry backed by a fresh store (not installed globally).
    pub fn new_local() -> Self {
        Self {
            catalog: Arc::new(tokio::sync::RwLock::new(PluginStore::new())),
        }
    }

    // list all tools (across all plugins, or on a given plugin)
    pub async fn tools(&self, plugin_id: Option<&str>) -> anyhow::Result<Vec<Tool>> {
        let guard = self.catalog.read().await;

        let tools: Vec<Tool> = match plugin_id {
            None => guard.tool_to_def.values().cloned().collect(),
            Some(id) => guard
                .tool_to_plugin
                .iter()
                .filter(|(_, owner_id)| owner_id == &id)
                .filter_map(|(tool_name, _)| guard.tool_to_def.get(tool_name).cloned())
                .collect(),
        };

        Ok(tools)
    }

    /// Calls a registered plugin handler with the given input.
    /// Clones the handler while holding the lock and invokes it outside to avoid blocking.
    pub async fn call(&self, id: &str, input: &Value) -> anyhow::Result<Value> {
        let handler = {
            let guard = self.catalog.read().await;
            guard.tool_to_handler.get(id).cloned()
        };

        match handler {
            Some(h) => match h(input.clone()).await {
                Ok(result) => Ok(result),
                Err(err) => Err(anyhow::anyhow!("Plugin handler error: {:?}", err)),
            },
            None => Err(anyhow::anyhow!("No handler registered for plugin '{}'", id)),
        }
    }
}
