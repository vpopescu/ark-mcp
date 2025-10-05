/// Implementation of the plugin management API.
///
/// This module provides HTTP handlers for managing plugins in the Ark server.
/// It includes endpoints for listing, retrieving, creating, and deleting plugins,
/// as well as executing plugin tools.
///
/// # Endpoints
///
/// - `GET /api/plugins` - Get a list of all plugins
/// - `GET /api/plugins/:id` - Get a specific plugin by ID
/// - `POST /api/plugins` - Register a new plugin
/// - `DELETE /api/plugins/:id` - Unregister a plugin by ID
/// - `POST /api/plugins/:id/tools/:tool_id` - Execute a tool on a plugin
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use hyper::StatusCode;
use serde_json::{Value, json};

use std::sync::Arc;
use std::time::Instant;

use crate::{
    config::plugins::ArkPlugin, plugins::builtin::BUILTIN_PLUGIN_ID,
    server::service::StandardizedResponse, state::ArkState,
};

/// Retrieves a list of all registered plugins.
///
/// # Endpoint
/// `GET /api/plugins`
///
/// # Returns
/// A JSON array of plugin configurations with tools.
pub async fn get_plugins(State(state): State<Arc<ArkState>>) -> impl IntoResponse {
    let start = Instant::now();
    tracing::debug!("API: GET /api/plugins");

    let catalog = state.plugin_registry.catalog.read().await;
    // Build plugin response with tools
    let mut plugins_object = json!({});

    for plugin in catalog.plugin_to_config.values() {
        // Get tools specifically for this plugin
        let tools = match state.plugin_registry.tools(Some(&plugin.name)).await {
            Ok(tools_vec) => tools_vec
                .into_iter()
                .map(|tool| {
                    json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema
                    })
                })
                .collect::<Vec<_>>(),
            Err(_) => Vec::new(),
        };

        let plugin_data = json!({
            "name": plugin.name,
            "description": plugin.manifest.as_ref().and_then(|m| m.config.as_ref())
                .and_then(|c| c.get("description").cloned())
                .unwrap_or_else(|| "No description".to_string()),
            "tools": tools,
            "url": plugin.url,
            "insecure": plugin.insecure,
            "manifest": plugin.manifest.as_ref().map(|m| json!({
                "wasm": m.wasm,
                "memory": m.memory,
                "config": m.config,
                "allowed_hosts": m.allowed_hosts,
                "allowed_paths": m.allowed_paths
            })).unwrap_or(json!(null))
        });

        if let Some(obj) = plugins_object.as_object_mut() {
            obj.insert(plugin.name.clone(), plugin_data);
        }
    }

    let response = (StatusCode::OK, Json(plugins_object)).into_response();
    let status = response.status().as_u16();
    let latency_ms = start.elapsed().as_millis() as f64;
    crate::metrics::record_api_http("/api/plugins", "GET", status, latency_ms);
    response
}

/// Retrieves a specific plugin by its ID.
///
/// # Endpoint
/// `GET /api/plugins/:id`
///
/// # Parameters
/// - `plugin_id`: The ID of the plugin to retrieve
///
/// # Returns
/// The plugin's tool set as JSON, or an error if not found.
pub async fn get_plugin_by_id(
    State(state): State<Arc<ArkState>>,
    Path(plugin_id): Path<String>,
) -> impl IntoResponse {
    let start = Instant::now();
    tracing::debug!("API: GET /api/plugin/{}", plugin_id);

    let response = match state.plugin_registry.tools(Some(&plugin_id)).await {
        Ok(toolset) => match serde_json::to_value(toolset) {
            Ok(val) => (StatusCode::OK, Json(val)),
            Err(e) => {
                tracing::error!("Failed to retrieve responses: {:?}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    StandardizedResponse::as_error("Failed to retrieve plugin", None),
                )
            }
        },
        Err(_e) => {
            tracing::debug!("Plugin '{}' not found: {:?}", plugin_id, _e);
            (
                StatusCode::NOT_FOUND,
                StandardizedResponse::as_error("Plugin not found", None),
            )
        }
    };

    let status = response.0.as_u16();
    let latency_ms = start.elapsed().as_millis() as f64;
    crate::metrics::record_api_http(
        &format!("/api/plugins/{}", plugin_id),
        "GET",
        status,
        latency_ms,
    );
    response.into_response()
}

/// Registers a new plugin.
///
/// # Endpoint
/// `POST /api/plugins`
///
/// # Parameters
/// - `payload`: JSON payload containing the plugin configuration as an [`ArkPlugin`]
///
/// # Returns
/// - 201 Created on success with a success message
/// - 500 Internal Server Error on failure
pub async fn create_plugin(
    State(state): State<Arc<ArkState>>,
    Json(payload): Json<ArkPlugin>,
) -> impl IntoResponse {
    let start = Instant::now();
    tracing::debug!("API: POST /api/plugins BODY={:?}", payload);

    let response = match crate::plugins::read_plugin_data(&payload).await {
        Ok(result) => {
            match state
                .register_plugin_with_executors(payload, result.toolset, result.executors)
                .await
            {
                Ok(_) => {
                    tracing::debug!("Plugin registered successfully");
                    (
                        StatusCode::CREATED,
                        Json(json!({"message": "Plugin registered successfully"})),
                    )
                }
                Err(e) => {
                    tracing::error!("Failed to register plugin: {:?}", e);
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        StandardizedResponse::as_error("Failed to register plugin", None),
                    )
                }
            }
        }
        Err(e) => {
            tracing::error!("Failed to read plugin data: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                StandardizedResponse::as_error("Failed to read plugin data", None),
            )
        }
    };

    let status = response.0.as_u16();
    let latency_ms = start.elapsed().as_millis() as f64;
    crate::metrics::record_api_http("/api/plugins", "POST", status, latency_ms);
    response.into_response()
}

/// Unregisters a plugin by its ID.
///
/// # Endpoint
/// `DELETE /api/plugins/:id`
///
/// # Parameters
/// - `plugin_id`: The ID of the plugin to delete
///
/// # Returns
/// - 204 No Content on success
/// - 400 Bad Request if trying to delete built-in plugin
/// - 404 Not Found if plugin doesn't exist
/// - 500 Internal Server Error on failure
///
/// # Notes
/// Prevents deletion of the built-in plugin.
pub async fn delete_plugin(
    State(state): State<Arc<ArkState>>,
    Path(plugin_id): Path<String>,
) -> impl IntoResponse {
    let start = Instant::now();
    tracing::debug!("API: DELETE /api/plugins/{}", plugin_id);

    if plugin_id == BUILTIN_PLUGIN_ID {
        tracing::debug!("Attempted to delete built-in plugin '{}'", plugin_id);
        let response = (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "Cannot delete built-in plugin" })),
        )
            .into_response();
        let status = response.status().as_u16();
        let latency_ms = start.elapsed().as_millis() as f64;
        crate::metrics::record_api_http(
            &format!("/api/plugins/{}", plugin_id),
            "DELETE",
            status,
            latency_ms,
        );
        return response;
    }

    let response = match state.unregister_plugin(plugin_id.as_str()).await {
        Ok(true) => {
            tracing::debug!("Plugin '{}' deleted successfully", plugin_id);
            (StatusCode::NO_CONTENT, Json(json!("OK"))).into_response()
        }
        Ok(false) => {
            tracing::debug!("Plugin '{}' was not found", plugin_id);
            (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "Plugin not found"
                })),
            )
                .into_response()
        }
        Err(e) => {
            tracing::error!("Failed to delete plugin '{}': {:?}", plugin_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "Failed to delete plugin"
                })),
            )
                .into_response()
        }
    };

    let status = response.status().as_u16();
    let latency_ms = start.elapsed().as_millis() as f64;
    crate::metrics::record_api_http(
        &format!("/api/plugins/{}", plugin_id),
        "DELETE",
        status,
        latency_ms,
    );
    response
}

/// Executes a tool on a specific plugin.
///
/// # Endpoint
/// `POST /api/plugins/:id/tools/:tool_id`
///
/// # Parameters
/// - `plugin_id`: The ID of the plugin
/// - `tool_id`: The ID of the tool to execute
/// - `payload`: JSON payload with tool arguments
///
/// # Returns
/// - 200 OK with the tool execution result
/// - 404 Not Found if plugin or tool doesn't exist, or tool doesn't belong to plugin
/// - 500 Internal Server Error on execution failure
pub async fn execute_plugin_tool(
    State(state): State<Arc<ArkState>>,
    Path((plugin_id, tool_id)): Path<(String, String)>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let start = Instant::now();
    tracing::debug!("API: POST /api/plugins/{}/tool/{}", plugin_id, tool_id);

    // Check if plugin exists
    let catalog = state.plugin_registry.catalog.read().await;
    if !catalog.plugin_to_config.contains_key(&plugin_id) {
        tracing::debug!("Plugin '{}' not found", plugin_id);
        let response = (
            StatusCode::NOT_FOUND,
            StandardizedResponse::as_error("Plugin not found", None),
        )
            .into_response();
        let status = response.status().as_u16();
        let latency_ms = start.elapsed().as_millis() as f64;
        crate::metrics::record_api_http(
            &format!("/api/plugins/{}/tools/{}", plugin_id, tool_id),
            "POST",
            status,
            latency_ms,
        );
        return response;
    }

    // Check if tool exists and belongs to the plugin
    if let Some(owner_plugin) = catalog.tool_to_plugin.get(&tool_id) {
        if owner_plugin != &plugin_id {
            tracing::debug!(
                "Tool '{}' belongs to plugin '{}' not '{}'",
                tool_id,
                owner_plugin,
                plugin_id
            );
            let response = (
                StatusCode::NOT_FOUND,
                StandardizedResponse::as_error("Tool not found for this plugin", None),
            )
                .into_response();
            let status = response.status().as_u16();
            let latency_ms = start.elapsed().as_millis() as f64;
            crate::metrics::record_api_http(
                &format!("/api/plugins/{}/tools/{}", plugin_id, tool_id),
                "POST",
                status,
                latency_ms,
            );
            return response;
        }
    } else {
        tracing::debug!("Tool '{}' not found", tool_id);
        let response = (
            StatusCode::NOT_FOUND,
            StandardizedResponse::as_error("Tool not found", None),
        )
            .into_response();
        let status = response.status().as_u16();
        let latency_ms = start.elapsed().as_millis() as f64;
        crate::metrics::record_api_http(
            &format!("/api/plugins/{}/tools/{}", plugin_id, tool_id),
            "POST",
            status,
            latency_ms,
        );
        return response;
    }
    drop(catalog); // Release the lock

    // Execute the tool
    let response = match state.plugin_registry.call(&tool_id, &payload).await {
        Ok(result) => {
            tracing::debug!("Tool '{}' executed successfully", tool_id);
            (StatusCode::OK, Json(result)).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to execute tool '{}': {:?}", tool_id, e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                StandardizedResponse::as_error("Tool execution failed", None),
            )
                .into_response()
        }
    };

    let status = response.status().as_u16();
    let latency_ms = start.elapsed().as_millis() as f64;
    crate::metrics::record_api_http(
        &format!("/api/plugins/{}/tools/{}", plugin_id, tool_id),
        "POST",
        status,
        latency_ms,
    );
    crate::metrics::record_tool_metrics(&plugin_id, &tool_id, latency_ms);
    response
}
