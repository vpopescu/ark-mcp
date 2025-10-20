//! Health check handlers for the Ark server.
//!
//! This module provides HTTP handlers for health and readiness checks.
//!
//! # Endpoints
//!
//! - `GET /livez` - Returns 200/OK if the server is alive (basic liveness check)
//! - `GET /readyz` - Returns 200/OK if the server is ready to serve requests
//!
//! # Response Format
//!
//! Both endpoints support content negotiation:
//! - `Accept: application/json` returns `{"status": "live|ready|not live|not ready"}`
//! - Default returns plain text `"live"`, `"ready"`, `"not live"`, or `"not ready"`
//!
//! # Notes
//!
//! Currently these checks are not comprehensive. Defining what it means to be
//! healthy and ready is still something to do. The paths can be changed in configuration.

use std::sync::Arc;

use axum::{extract::State, response::Response};
use hyper::{HeaderMap, StatusCode};
use serde_json::json;

use crate::state::ArkState;

/// Liveness check handler.
///
/// This endpoint indicates whether the server is running and can respond to requests.
/// It performs a basic check to ensure the server process is alive.
///
/// # Endpoint
/// `GET /livez`
///
/// # Parameters
/// - `state`: Application state containing liveness information
/// - `headers`: HTTP headers, used for content negotiation via Accept header
///
/// # Returns
/// - 200 OK with "live" if the server is alive
/// - 503 Service Unavailable with "not live" if the server is not alive
pub async fn livez(State(state): State<Arc<ArkState>>, headers: HeaderMap) -> Response {
    tracing::debug!("livez_handler invoked");

    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    let (status, text) = if state.is_alive() {
        (StatusCode::OK, "live")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not live")
    };

    let body = if accept.contains("application/json") {
        json!({ "status": text }).to_string()
    } else {
        text.to_string()
    };

    Response::builder()
        .status(status)
        .header(
            "Content-Type",
            if accept.contains("application/json") {
                "application/json"
            } else {
                "text/plain"
            },
        )
        .body(body.into())
        .unwrap()
}

/// Readiness check handler.
///
/// This endpoint indicates whether the server is ready to serve requests.
/// It checks if the server has completed initialization and is ready for traffic.
///
/// # Endpoint
/// `GET /readyz`
///
/// # Parameters
/// - `state`: Application state containing readiness information
/// - `headers`: HTTP headers, used for content negotiation via Accept header
///
/// # Returns
/// - 200 OK with "ready" if the server is ready
/// - 503 Service Unavailable with "not ready" if the server is not ready
pub async fn readyz(State(state): State<Arc<ArkState>>, headers: HeaderMap) -> Response {
    tracing::debug!("readyz_handler invoked");

    let accept = headers
        .get("accept")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Consider both application readiness and signer readiness
    let app_ready = state.is_ready();
    let signer_ready = state.is_signer_ready();

    let (status, text) = if app_ready && signer_ready {
        (StatusCode::OK, "ready")
    } else {
        tracing::debug!(
            "Server not ready: app_ready={}, signer_ready={}",
            app_ready,
            signer_ready
        );
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    };

    let body = if accept.contains("application/json") {
        json!({ "status": text}).to_string()
    } else {
        text.to_string()
    };

    Response::builder()
        .status(status)
        .header(
            "Content-Type",
            if accept.contains("application/json") {
                "application/json"
            } else {
                "text/plain"
            },
        )
        .body(body.into())
        .unwrap()
}
