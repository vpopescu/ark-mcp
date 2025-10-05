//! # Metrics HTTP Handler
//!
//! This module provides HTTP endpoint handling for metrics exposition.
//! It serves Prometheus-formatted metrics at the `/metrics` endpoint when
//! the `prometheus` feature is enabled.
//!
//! ## Features
//!
//! - **Prometheus Endpoint**: Serves metrics in Prometheus text format
//! - **Conditional Compilation**: Only compiled when `prometheus` feature is enabled
//! - **Graceful Degradation**: Returns appropriate responses when metrics are unavailable
//!
//! ## HTTP Responses
//!
//! - `200 OK`: Metrics successfully rendered
//! - `503 Service Unavailable`: Metrics recorder not initialized
//! - `404 Not Found`: Metrics feature not compiled in

use http_body_util::Full;
use hyper::Response;
use hyper::body::Bytes;

/// Global Prometheus handle for metrics rendering.
///
/// This static variable holds the Prometheus recorder handle once initialized.
/// Uses `OnceLock` for thread-safe, one-time initialization.
#[cfg(feature = "prometheus")]
static PROM_HANDLE: std::sync::OnceLock<metrics_exporter_prometheus::PrometheusHandle> =
    std::sync::OnceLock::new();

/// Sets the global Prometheus handle for metrics rendering.
///
/// This function should be called once during server initialization to provide
/// the metrics system with a handle for rendering Prometheus-formatted output.
///
/// # Arguments
/// * `handle` - The Prometheus handle from the metrics exporter
///
/// # Panics
/// This function will panic if called more than once, as the handle can only be set once.
#[cfg(feature = "prometheus")]
pub(crate) fn set_prom_handle(handle: metrics_exporter_prometheus::PrometheusHandle) {
    let _ = PROM_HANDLE.set(handle);
}

/// Builds an HTTP response containing Prometheus metrics.
///
/// Returns metrics in the standard Prometheus text format (version 0.0.4).
/// This endpoint is used by Prometheus scrapers to collect metrics.
///
/// # Returns
/// An HTTP response containing:
/// - `200 OK` with metrics data when available
/// - `503 Service Unavailable` when metrics recorder is not initialized
/// - `404 Not Found` when Prometheus feature is not compiled in
///
/// # Content Type
/// `text/plain; version=0.0.4; charset=utf-8`
pub fn make_metrics_response() -> Response<Full<Bytes>> {
    #[cfg(feature = "prometheus")]
    {
        use hyper::{StatusCode, header};

        tracing::debug!("Metrics requested");
        if let Some(handle) = PROM_HANDLE.get() {
            // Render current metrics when the handle has been initialized.
            let body = handle.render();
            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, "text/plain; version=0.0.4")
                .body(Full::new(Bytes::from(body)))
                .expect("Failed to build metrics response");
        }
        // Return 503 when the Prometheus recorder has not been initialized yet.
        Response::builder()
            .status(StatusCode::SERVICE_UNAVAILABLE)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Full::new(Bytes::from(
                "prometheus recorder not initialized",
            )))
            .expect("Failed to build service unavailable response")
    }
    #[cfg(not(feature = "prometheus"))]
    {
        // Return 404 when Prometheus support is not compiled in.
        tracing::warn!("Metrics endpoint called with metrics disabled");
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(Full::new(Bytes::from_static(b"metrics disabled")))
            .expect("Failed to build not found response")
    }
}
