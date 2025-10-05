//! Built-in plugin implementation for the Ark MCP server.
//!
//! This module provides the built-in "echo" tool that simply returns the input message
//! as output. It serves as an example plugin and basic functionality test for the
//! MCP server infrastructure.
//!
//! # Built-in Tools
//!
//! - **echo**: Returns the input message unchanged, useful for testing and basic functionality.

use crate::config::plugins::{ArkPlugin, PluginManifest};
use crate::plugins::registry::ToolProvider;
use crate::plugins::{ToolContentBlock, ToolResult, ToolSet};
use rmcp::ErrorData;
use rmcp::model::Tool;
use rmcp::serde_json::Value;
use serde_json::Map;
use std::collections::BTreeMap;
use std::{borrow::Cow, sync::Arc};

/// Identifier for the built-in plugin in the plugin registry.
pub const BUILTIN_PLUGIN_ID: &str = "__BUILTIN__";

/// Built-in plugin implementation providing basic echo functionality.
///
/// This plugin serves as the default tool provider for the Ark MCP server,
/// offering a simple echo tool for testing and basic functionality verification.
#[derive(Debug, Clone)]
pub struct BuiltinPlugin;

#[async_trait::async_trait]
impl ToolProvider for BuiltinPlugin {
    /// Returns the complete tool set provided by this plugin.
    fn toolset(&self) -> ToolSet {
        self.toolset().clone()
    }

    /// Executes a tool with the given input parameters.
    ///
    /// For the built-in echo tool, this extracts the "message" field from the input
    /// and returns it as both structured content and text content.
    ///
    /// # Arguments
    /// * `input` - JSON value containing tool arguments
    ///
    /// # Returns
    /// A result containing the tool execution output or an error.
    async fn call(&self, input: &Value) -> Result<Value, ErrorData> {
        // Extract the "message" field from input, defaulting to empty string
        let message = input
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();

        let block = ToolContentBlock {
            kind: "text",
            text: message,
        };

        let result = ToolResult {
            content: vec![block],
            structured_content: message,
            is_error: false,
        };

        rmcp::serde_json::to_value(&result)
            .map_err(|_| ErrorData::invalid_params("failed to serialize tool result", None))
    }
}

impl BuiltinPlugin {
    /// Creates the tool set definition for the built-in plugin.
    ///
    /// This method defines the "echo" tool with its input schema, description,
    /// and metadata. The tool accepts a single "message" parameter of type string.
    ///
    /// # Returns
    /// A `ToolSet` containing the echo tool definition.
    pub fn toolset(&self) -> ToolSet {
        // Define the input schema for the message parameter
        let mut properties = Map::new();
        properties.insert(
            "message".to_string(),
            Value::Object({
                let mut obj = Map::new();
                obj.insert("type".to_string(), Value::String("string".to_string()));
                obj
            }),
        );

        // Define the complete input schema as an object
        let mut input_schema = Map::new();
        input_schema.insert("type".to_string(), Value::String("object".to_string()));
        input_schema.insert("properties".to_string(), Value::Object(properties));

        // Create the echo tool definition
        let tool = Tool {
            name: Cow::Borrowed("echo"),
            title: Some("Echo Tool".to_string()),
            description: Some(Cow::Borrowed("Returns the input message as output.")),
            input_schema: Arc::new(input_schema),
            output_schema: None,
            annotations: None,
            icons: None,
        };

        ToolSet {
            name: tool.name.clone().into_owned(),
            tools: vec![tool],
        }
    }

    /// Converts a tool provider into a plugin configuration tuple.
    ///
    /// This utility method creates a plugin configuration and toolset pair
    /// that can be registered with the plugin system. It's primarily used
    /// for built-in plugins that don't require external loading.
    ///
    /// # Type Parameters
    /// * `P` - The tool provider type that implements `ToolProvider`
    ///
    /// # Arguments
    /// * `plugin_id` - Unique identifier for the plugin
    /// * `plugin` - Reference to the tool provider instance
    ///
    /// # Returns
    /// A tuple containing the plugin configuration and its toolset.
    pub fn as_plugin_config<P: ToolProvider>(plugin_id: &str, plugin: &P) -> (ArkPlugin, ToolSet) {
        let toolset = plugin.toolset();

        let manifest = PluginManifest {
            wasm: None,
            memory: None,
            config: Some(BTreeMap::from([
                ("builtin".to_string(), "true".to_string()),
                ("tool".to_string(), toolset.name.clone()),
            ])),
            allowed_hosts: None,
            allowed_paths: None,
        };

        let config = ArkPlugin::new(plugin_id.to_string(), Some(manifest));
        (config, toolset)
    }
}
