/// Implementation of the MCP server handler.
/// This server implements the Streamable-HTTP protocol (/mcp).
use std::sync::Arc;
use std::time::Instant;

use crate::server::constants::{
    MCP_SERVER_INFO_NAME, MCP_SERVER_INFO_TITLE, MCP_SERVER_INFO_URL, MCP_SERVER_INFO_VERSION,
};
use crate::state::ArkState;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{Implementation, ServerCapabilities};
use rmcp::model::{ListToolsResult, PaginatedRequestParam, ServerInfo};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer};
use serde_json::{Map, Value};
use tokio::runtime::Handle;
use tokio::task::block_in_place;

/// Handler for MCP (Model Context Protocol) server operations.
///
/// This struct implements the `ServerHandler` trait from the `rmcp` crate,
/// providing the core logic for handling MCP requests such as listing tools
/// and calling tools. It holds a reference to the application state.
pub(crate) struct McpHandler {
    /// Shared application state containing plugin registry and configuration.
    pub(crate) state: Arc<ArkState>,
}

// Implement ServerHandler interface
impl ServerHandler for McpHandler {
    /// Handle MCP initialization request.
    ///
    /// Returns server capabilities and information to complete the initialization handshake.
    fn initialize(
        &self,
        _request: rmcp::model::InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::InitializeResult, ErrorData>> + Send + '_ {
        let start = Instant::now();
        tracing::debug!("McpHandler: initialize");
        async move {
            // Return the same server info as get_info
            let server_info = self.get_info();
            let result = Ok(rmcp::model::InitializeResult {
                capabilities: server_info.capabilities,
                server_info: server_info.server_info,
                instructions: None,
                protocol_version: rmcp::model::ProtocolVersion::V_2024_11_05,
            });
            let latency_ms = start.elapsed().as_millis() as f64;
            crate::metrics::record_mcp_call("initialize", latency_ms);
            result
        }
    }

    /// Implementation of the ServerHandler trait for MCP operations.
    ///
    /// This implementation provides the necessary methods to handle MCP protocol
    /// requests, including server information, tool listing, and tool execution.
    /// All operations are performed asynchronously where appropriate.
    /// Returns the server information and capabilities.
    ///
    /// This method provides the server's identity and supported features
    /// as required by the MCP protocol. It synchronously fetches the list
    /// of available tools to determine capabilities.
    ///
    /// # Returns
    /// A `ServerInfo` struct containing server metadata and capabilities.
    fn get_info(&self) -> ServerInfo {
        tracing::debug!("McpHandler: get_info");

        // Fetch tools synchronously using block_in_place since get_info is not async
        // This is necessary because the rmcp ServerHandler trait requires get_info to be sync
        let tools =
            block_in_place(|| Handle::current().block_on(self.state.clone().get_tools(None)))
                .unwrap_or_default();

        let mut completions = Map::new();
        for tool in &tools {
            completions.insert(format!("tool.{}", tool.name), Value::Bool(true));
        }

        // Build capabilities based on available tools
        let capabilities = ServerCapabilities {
            experimental: None,
            logging: None,
            completions: Some(completions),
            prompts: None,
            resources: None,
            tools: if tools.is_empty() {
                None
            } else {
                Some(rmcp::model::ToolsCapability {
                    list_changed: Some(true),
                })
            },
        };

        ServerInfo {
            capabilities,

            server_info: Implementation {
                name: MCP_SERVER_INFO_NAME.to_owned(),
                title: Some(MCP_SERVER_INFO_TITLE.to_owned()),
                version: MCP_SERVER_INFO_VERSION.to_owned(),
                icons: None, // TODO: add server icon here
                website_url: Some(MCP_SERVER_INFO_URL.to_owned()),
            },
            ..Default::default()
        }
    }

    /// Returns a list of tools available on the server.
    ///
    /// Constructs a flat list where each registered plugin that has a handler
    /// is exposed as a tool (name == plugin id).
    ///
    /// # Arguments
    /// * `_request` - Optional pagination parameters (currently unused)
    /// * `_context` - Request context (currently unused)
    ///
    /// # Returns
    /// A future resolving to `ListToolsResult` or an `ErrorData`.
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let start = Instant::now();
        tracing::debug!("McpHandler: list_tools");
        async move {
            let catalog = self.state.plugin_registry.catalog.read().await;
            let mut tools = Vec::new();

            for (tool_name, tool_def) in catalog.tool_to_def.iter() {
                if self.state.plugin_registry.has_handler(tool_name) {
                    tools.push(tool_def.clone());
                }
            }

            let result = Ok(ListToolsResult {
                tools,
                next_cursor: None,
            });
            let latency_ms = start.elapsed().as_millis() as f64;
            crate::metrics::record_mcp_call("list_tools", latency_ms);
            result
        }
    }

    /// Dispatches a tool call to the plugin registry.
    ///
    /// We expect clients to use the tool name that matches the plugin id
    /// (as returned from list_tools above). The method converts the request
    /// arguments to JSON, calls the plugin, and deserializes the result
    /// into a proper MCP `CallToolResult`.
    ///
    /// # Arguments
    /// * `request` - The tool call request parameters
    /// * `_context` - Request context (currently unused)
    ///
    /// # Returns
    /// A future resolving to `CallToolResult` or an `ErrorData`.
    ///
    /// # Details
    /// This method handles the conversion between MCP protocol and plugin
    /// execution, including error handling for invalid responses or
    /// execution failures.
    fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParam,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<rmcp::model::CallToolResult, rmcp::ErrorData>>
    + Send
    + '_ {
        let start = Instant::now();
        tracing::debug!("McpHandler: call_tool");

        async move {
            // Convert optional arguments (JsonObject) to a serde_json::Value
            let args_value = match request.arguments {
                Some(map) => rmcp::serde_json::Value::Object(map),
                None => rmcp::serde_json::Value::Object(Default::default()),
            };

            // Use the PluginRegistry to execute the handler (plugin id == request.name)
            let registry = &self.state.plugin_registry;
            let plugin_id = request.name.as_ref();

            let result = match registry.call(plugin_id, &args_value).await {
                Ok(val) => {
                    // The plugin returns a CallToolResult as JSON Value, deserialize it
                    match serde_json::from_value::<rmcp::model::CallToolResult>(val) {
                        Ok(result) => Ok(result),
                        Err(e) => Err(rmcp::ErrorData::invalid_params(
                            format!("Plugin returned invalid CallToolResult: {}", e),
                            None,
                        )),
                    }
                }
                Err(err) => {
                    // Return a proper MCP error CallToolResult
                    Ok(rmcp::model::CallToolResult {
                        content: vec![rmcp::model::Content {
                            raw: rmcp::model::RawContent::Text(rmcp::model::RawTextContent {
                                text: format!("Plugin execution failed: {}", err),
                                meta: None,
                            }),
                            annotations: None,
                        }],
                        is_error: Some(true),
                        meta: None,
                        structured_content: None,
                    })
                }
            };

            let latency_ms = start.elapsed().as_millis() as f64;
            crate::metrics::record_mcp_call("call_tool", latency_ms);

            // Also record tool metrics if we have a plugin_id
            if let Ok(call_result) = &result
                && call_result.is_error != Some(true)
            {
                // Only record successful tool calls
                crate::metrics::record_tool_metrics(plugin_id, plugin_id, latency_ms);
            }

            result
        }
    }
}
